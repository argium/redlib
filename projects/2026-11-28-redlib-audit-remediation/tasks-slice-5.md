# Redlib Audit Remediation, Slice 5 — Task List

- **Status:** Complete
- **Author:** Argi
- **Created:** 2026-11-28
- **Language:** Rust, JavaScript (Node `node:test` for the scripts), Askama HTML templates, CSS
- **Source:** [plan.md](./plan.md), Slice 5 (S5): Frontend JS Reliability, OpenSearch, Keyboard Accessibility & Valid HTML

## Summary

Several browser features fail without telling anyone: a failing HLS stream is retried forever, the update checker throws on every outdated instance, Copy does nothing over plain HTTP, the OpenSearch descriptor points at `localhost:8080`, and link previews get relative image URLs. Keyboard and screen-reader users hit labels that connect to nothing, icon buttons with no name, spoilers and blurred posts that only a mouse can reveal, and no way past the navbar. These tasks fix each one against the real scripts, routes and templates, and leave the markup free of the duplicate IDs, stray `<legend>`s and nested interactive elements the audit found.

## Starting Context

Existing code the tasks build on (verified against `f81dcf4`, the Slice 4 head; do not re-explore):

- `static/playHLSVideo.js:70-82` — `Hls.Events.ERROR` calls `hls.startLoad()` / `hls.recoverMediaError()` on every fatal error. The script reads `#video_quality` for its default level (`:3`) and builds the quality `<select>` without a name (`:89`). The cloned `<video>` has its `<source>` elements removed (`:23-27`), so the MP4 URL has to be remembered first.
- `static/check_update.js:24,27` — `document.getElementById('error-446').remove()`. No template has that element. `templates/error.html:9,11` gives two headings `id="update-status"`.
- `static/copy.js` — `navigator.clipboard.writeText(...)` with no fallback and no feedback. `templates/settings.html` loads it next to `#copy`.
- `src/main.rs:67` — `opensearch()` serves `static/opensearch.xml` (hard-coded `https://localhost:8080`) with a 14-day cache. `src/utils.rs:1616` — `to_absolute_url` prefixes `REDLIB_FULL_URL` only (used by RSS).
- `templates/post.html:25-38` — `og:image`, `twitter:image`, `og:video`, `og:url` use relative paths. `templates/post.html:36-38` nests the `highlighted.js` include inside the non-image, non-video branch.
- `templates/base.html:31` — `<div id="video_quality">` sits in `<head>`; `templates/settings.html:57` reuses the id on the `<select>`. `post_sort` and three checkboxes have no `id`.
- `src/utils.rs` — `rewrite_urls` post-processes every Reddit body, comment, wiki and sidebar HTML string. `parse_post_code_fence_keeps_spoilers_and_autolinks` (`src/utils.rs`) pins the exact spoiler markup.
- `static/style.css:107` global `:focus-visible`, `:834` mobile `#search:focus { outline: 0 }`, `:855-865` `overflow-y: hidden` on `#sort_options`, `#listing_options`, pagination, `:1156-1162` `.post_blurred` reveal on `:hover` only, `:1795-1810` `.md-spoiler-text` reveal on `:hover` only.
- Tests render templates offline from their module's struct (see `flaired_post()` and the `flair_gallery_and_prefs_versioning_links_*` tests); `serve_app()` and `fetch()` in `src/main.rs` tests serve the real route table.

Conventions and gotchas:

- Tabs (`rustfmt.toml`). Windows builds need `cmake` and `nasm` on `PATH` (see [plan.md Prerequisites](./plan.md#prerequisites)).
- Script tests run with `node --test tests/js/` (run on Node 26, no packages). Each test loads the real script into a `node:vm` context with hand-written `document`, `fetch`, `Hls` and clipboard fakes.
- Reddit answers 403 from this network, so no check uses live Reddit. Keyboard and focus behaviour is checked in a browser with Playwright against rendered fixture pages and `/settings`.
- The [modern-web-guidance](https://github.com/GoogleChrome/modern-web-guidance) `accessibility` and `copy-button` guides apply: native elements first, `aria-label` (not `title`) names icon-only controls, a `role="status"` element beside the button announces copy results, and `outline-offset` keeps focus rings visible.
- No new crates. `#48`'s encoded-link half was Slice 4; this slice only does its markup half.
- Gallery counters, alt text and the empty caption `<p>` are Slice 6 (`#42` gallery UX).

## Tasks

### Task 1: A failing HLS stream stops retrying and falls back to the MP4

#### Problem

When `hls.js` reports a fatal network or media error, `playHLSVideo.js` restarts loading or recovers the media straight away, every time. An expired `/hls/` stream or a `403` from Reddit therefore loops against the instance and Reddit forever, and the visitor never learns playback failed.

#### Solution

- Each player allows three recoveries per error type, with waits of 1 s, 2 s and 4 s for network errors.
- The fourth fatal error, or any other fatal type, destroys the `Hls` instance, removes the quality selector, loads the MP4 `<source>` into the video and shows a short message.
- With no MP4 source the message says playback failed.
- The page keeps working without the script's help: the video still has its `controls`.

#### Notes

- **Location:** `static/playHLSVideo.js`, `tests/js/playHLSVideo.test.js`, `tests/js/support/page.js`

- **Verify:** `node --test tests/js/` passes when the retry, backoff, fallback and message tests pass.

- Boundary: the real script in a `node:vm` context with fake `Hls`, `document` and timers. Real `hls.js` and real streams are not exercised.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `tests/js/playHLSVideo.test.js`:

- `network errors are retried three times with doubling waits, then the player is destroyed`: five fatal `NETWORK_ERROR`s give three `startLoad` calls after waits of 1000, 2000 and 4000 ms, then one `destroy`.
- `media errors are recovered three times, then the player is destroyed`.
- `after giving up, the video plays the MP4 source and says so`: `video.src` is the MP4 URL, a `.hls_error` message with text is appended.
- `without an MP4 source the message says the video failed`.

Red failure: `destroy` was never called (the fatal error was retried on every event, `startLoad` called 5 times).

**Implementation guidance (green)**:

- Keep `retries = { network: 0, media: 0 }` beside `initializeHls`; `setTimeout(hls.startLoad.bind(hls), 1000 * 2 ** retries.network++)`.
- Read the MP4 URL from `oldVideo.querySelector("source[type='video/mp4']")` before the sources are removed.
- `giveUp()` destroys, removes the selector, sets `newVideo.src`, calls `load()`/`play()` and appends `<p class="hls_error" role="status">`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M5.1

</details>

- [x] Task 1 complete

---

### Task 2: The update checker reports commit lag instead of throwing

#### Problem

On an instance that is behind, `check_update.js` calls `document.getElementById('error-446').remove()`. No page has that element, so the call throws, the `catch` overwrites the message with `Error checking update status: TypeError`, and the visitor never sees how many commits behind the instance is. `error.html` also gives two headings the same `id="update-status"`.

#### Solution

- Delete the two `error-446` lines (the element does not exist in any template).
- Give the second error-page heading no `id`; the script only writes to the first.

#### Notes

- **Location:** `static/check_update.js`, `templates/error.html`, `tests/js/check_update.test.js`

- **Verify:** `node --test tests/js/` passes when the outdated and up-to-date cases pass; the duplicate id is covered by Task 8's page checks.

- Boundary: the real script with a fake `fetch` returning an Atom feed, and a minimal `DOMParser` fake.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `tests/js/check_update.test.js`:

- `an outdated instance reports how many commits it is behind`: local commit third in the feed gives text containing `2 commits old` and not `Error`.
- `a commit not in the feed reports "at least N commits old"`.
- `the current commit reports up to date` (control, passes before and after).

Red failure: status was `⚠️ Error checking update status: TypeError: Cannot read properties of null (reading 'remove')`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 1 (JS test harness)

- **Supports:** M5.1

</details>

- [x] Task 2 complete

---

### Task 3: Copy works over plain HTTP and says what happened

#### Problem

`copy.js` calls `navigator.clipboard.writeText`, which does not exist on plain HTTP or `.onion` instances, so the click throws inside an async handler and nothing happens. On HTTPS the copy works but the button gives no sign of it.

#### Solution

- Use the Clipboard API when present, otherwise select the field and call `document.execCommand('copy')`.
- Write "Copied" or "Copy failed" into a `role="status"` element beside the button and clear it after two seconds.
- `templates/settings.html` gains `<span id="copy_status" role="status"></span>` after the button.

#### Notes

- **Location:** `static/copy.js`, `templates/settings.html`, `tests/js/copy.test.js`

- **Verify:** `node --test tests/js/` passes when the three copy cases pass.

- Boundary: the real script with fake `navigator`, `document` and timers. A real clipboard and screen reader are not exercised.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `tests/js/copy.test.js`:

- `without navigator.clipboard the field is selected and copied with execCommand` and the status reads `Copied`.
- `with navigator.clipboard the text is written and the status reads Copied`.
- `a refused copy says Copy failed`; `the status clears after two seconds`.
- `src/settings.rs`: `template_a11y_and_html_settings_copy_result_is_announced_beside_the_button` — the `role="status"` element follows the Copy button and sits outside it.

Red failure: `TypeError: Cannot read properties of undefined (reading 'writeText')`.

**Implementation guidance (green)**:

- `try { await navigator.clipboard.writeText(text) } catch { legacy }` mirrors the `copy-button` guide; the legacy path selects the existing readonly input.
- Clear the pending timer on each click rather than stacking timers; do not disable the button.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 1 (JS test harness)

- **Supports:** M5.1

</details>

- [x] Task 3 complete

---

### Task 4: `/opensearch.xml` names the instance it came from

#### Problem

The OpenSearch descriptor is a static file with `https://localhost:8080` baked in and a 14-day cache, so "Add search engine" on every deployed instance searches `localhost`.

#### Solution

- `templates/opensearch.xml` replaces `static/opensearch.xml`; Askama escapes the base URL.
- `utils::request_base_url(&req)` returns `REDLIB_FULL_URL` (trailing `/` trimmed) when set, otherwise `{scheme}://{Host}`. The scheme is `X-Forwarded-Proto` when it is `http` or `https`, else `http`.
- A request with no usable `Host` and no `REDLIB_FULL_URL` gets `400`. `Host` is usable only as a lower-cased DNS name or IPv4 address (no empty or hyphen-edged label, so no trailing dot) or a bracketed IPv6 literal, with an optional port of 1 to 65535.
- With `REDLIB_FULL_URL` set the answer keeps `public, max-age=1209600, s-maxage=86400`. Otherwise it is `private, max-age=300` with `Vary: Host, X-Forwarded-Proto`, so a shared cache never stores a request-derived descriptor.
- `X-Forwarded-Host` and `Forwarded` are ignored; the README says to set `REDLIB_FULL_URL` behind a proxy that rewrites `Host`.

#### Notes

- **Location:** `src/main.rs`, `src/utils.rs`, `templates/opensearch.xml` (moved from `static/`)

- **Verify:** `cargo test --locked opensearch_and_og` passes.

- Boundary: the real route table on a loopback port with chosen `Host` headers; `request_base_url` on built requests.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**:

- `src/main.rs`: `opensearch_and_og_descriptor_uses_the_request_host` — `Host: redlib.example` gives `http://redlib.example/search` (and `https://` with `X-Forwarded-Proto: https`) in the template, image and search-form URLs, and no `localhost:8080`.
- `src/main.rs`: `opensearch_and_og_descriptor_rejects_an_unusable_host` — `Host: evil"><x` and `Host: user@redlib.example` answer `400`.
- `src/main.rs`: `opensearch_and_og_descriptor_built_from_request_headers_is_not_shared_cacheable` and `..._from_the_configured_url_is_publicly_cacheable`.
- `src/utils.rs`: `opensearch_and_og_request_base_url_rejects_hosts_that_are_not_host_and_port` and `..._accepts_names_addresses_and_ports_in_lower_case`; `opensearch_and_og_configured_urls_cannot_inject_markup_into_the_descriptor` (and `post.rs` `..._meta_urls_escape_a_configured_base`) pin Askama's escaping.
- `src/main.rs`: `opensearch_and_og_descriptor_prefers_the_configured_full_url` (`sealed_test`, `REDLIB_FULL_URL=https://libred.example/`): URLs start `https://libred.example/search`.
- `src/utils.rs`: `opensearch_and_og_request_base_url_comes_from_the_host`, `..._follows_the_forwarded_scheme`, `..._is_unknown_without_a_usable_host` and `..._prefers_the_configured_full_url`.

Red failure: body contained `https://localhost:8080/search`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M5.2

</details>

- [x] Task 4 complete

---

### Task 5: Open Graph and Twitter tags carry absolute URLs

#### Problem

`og:image`, `twitter:image`, `og:video`, `og:url` and `twitter:url` hold relative paths such as `/thumb/...`, which chat apps and link-preview crawlers ignore.

#### Solution

- `PostTemplate` gains `base_url` (from `request_base_url`) and an `absolute()` method: a value starting with a single `/` gets the base prefixed, everything else (empty, already absolute, protocol-relative) is left alone.
- With no known base the tags stay relative, as before.

#### Notes

- **Location:** `src/post.rs`, `src/utils.rs`, `templates/post.html`

- **Verify:** `cargo test --locked opensearch_and_og` passes.

- Boundary: `PostTemplate` rendered from fixtures.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `src/post.rs`:

- `opensearch_and_og_meta_urls_are_absolute_when_the_base_is_known` — image, video and gif posts with base `https://redlib.example`: `og:image`, `twitter:image`, `og:video`, `og:url` start with it.
- `opensearch_and_og_meta_urls_stay_relative_without_a_base`.
- `src/utils.rs`: `opensearch_and_og_absolute_url_only_prefixes_site_paths`.

Red failure: `og:image` content was `/thumb/...` (`content="https://redlib.example/thumb/..."` not found).

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 4 (`request_base_url`)

- **Supports:** M5.2

</details>

- [x] Task 5 complete

---

### Task 6: Labels, icon buttons and the page head are valid and named

#### Problem

`<div id="video_quality">` sits inside `<head>` and repeats the id of the settings `<select>`, so that label points at nothing. `post_sort` and three settings checkboxes have no `id`, so their labels and click targets are disconnected. The search arrow buttons, the mobile Reddit link (its text hides at 800 px), the JS-built HLS quality selector and several selects have no accessible name. The gallery's outbound `<p>` is never closed.

#### Solution

- `<body data-video-quality="...">` carries the preference; `playHLSVideo.js` reads `document.body.dataset.videoQuality`.
- Add the four missing `id`s, `aria-label`s on the arrow buttons, title-only selects, search inputs and the import field, a `<title>` inside the Reddit icon, and `aria-label="Video quality"` on the HLS selector.
- Close the gallery `<p>`.
- The shared post fixtures gain titles, so the post-title links have text to name them.

#### Notes

- **Location:** `templates/base.html`, `templates/settings.html`, `templates/utils.html`, `templates/subreddit.html`, `templates/user.html`, `templates/search.html`, `templates/post.html`, `static/playHLSVideo.js`, `src/test_support.rs`

- **Verify:** `cargo test --locked template_a11y_and_html` and `node --test tests/js/` pass.

- Boundary: pages rendered from fixtures and checked with the HTML helpers in `test_support`; accessible names follow the `accessibility` guide (`aria-label`/`<label>`, never `title` or `placeholder` alone).

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**:

- `html_checks::assert_accessible(page, html)` (new `src/html_checks.rs`) reports label targets that are missing or not controls, controls and links without a name, `<head>` children that are not head elements, and unbalanced `<p>`. Four tests drive the checkers with defective and clean snippets, so a checker that finds nothing cannot pass unnoticed.
- `template_a11y_and_html_*` in `post.rs`, `settings.rs`, `subreddit.rs`, `user.rs`, `search.rs`, `duplicates.rs` and `utils.rs` (error and info pages) assert no problems, plus `template_a11y_and_html_video_quality_travels_on_the_body` and `template_a11y_and_html_nav_icons_name_themselves`.
- `tests/js/playHLSVideo.test.js`: `the quality preference comes from the body` and `the quality selector has an accessible name and goes away when the player gives up`.

Red failures: `label for="post_sort" has no control`, `<div> inside <head>`, `button.submit has no accessible name`, `p opened 3 times, closed 2`, and a `TypeError` reading `data-value` of `null`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 1

- **Supports:** M5.3

</details>

- [x] Task 6 complete

---

### Task 7: Keyboard users can see focus, reveal spoilers and skip the navbar

#### Problem

Mobile `#search:focus` removes the focus ring, `overflow-y: hidden` clips the rings on the sort and listing tabs, and the selected tab's ring matches its own fill. Spoiler text and blurred NSFW or spoiler media reveal on `:hover` only. There is no way past the navbar, and feeds, search and settings have no `<h1>`.

#### Solution

- Remove `#search:focus { outline: 0 }`; draw tab rings inside the tab with a negative `outline-offset` and a contrasting colour on the selected tab; draw the search form's children's rings inside them (the form clips them). Pagination links need no change: their ring sits outside the link and is not clipped (checked in the browser).
- `.post_blurred:focus-within` reveals blurred parts; `rewrite_urls` adds `tabindex="0"` to `.md-spoiler-text` and the stylesheet reveals on `:focus`.
- `base.html` starts with `<a class="skip-link" href="#main-content">Skip to content</a>`; every `<main>` has `id="main-content"`. The skip link shows on focus.
- Subreddit, search, settings and wiki pages get a visually hidden `<h1>`; `info.html`'s heading becomes an `<h1>`; the sidebar `#sub_title` becomes an `<h2>`; the leave-Redlib popup heading becomes an `<h2>`.

#### Notes

- **Location:** `static/style.css`, `templates/*.html`, `src/utils.rs`

- **Verify:** `cargo test --locked a11y_markup` passes; keyboard walkthrough recorded in Acceptance Evidence.

- Boundary: rendered markup for the skip link, `<main>`, `<h1>` count and spoiler `tabindex`; focus visibility and reveal were checked in a browser with Playwright (no CSS-source assertions).

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**:

- `html_checks::assert_landmarks(page, html)`; `a11y_markup_*` in each page module (post, subreddit, home feed, wiki, wall, search, settings, profile, duplicates, error, info, NSFW landing): the first link in `<body>` is the skip link, `#main-content` exists once and is the `<main>`, the page has exactly one `<h1>`.
- `src/utils.rs`: `a11y_markup_spoiler_spans_can_take_keyboard_focus` — `rewrite_urls` turns `<span class="md-spoiler-text">` into one with `tabindex="0"`; the code-fence markdown test is updated to the same markup.

Red failures: `no skip link before the navbar`, `0 <h1> elements`, `spoiler span is not focusable`.

**Manual checks (Playwright)**: focus ring on mobile search, sort and listing tabs and pagination; Tab reveals a blurred post and a spoiler; the skip link appears on first Tab and moves focus into `<main>`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 6

- **Supports:** M5.4

</details>

- [x] Task 7 complete

---

### Task 8: `highlighted.js` loads on every thread and the markup stops repeating itself

#### Problem

`highlighted.js` only loads for text and link posts, so jumping to a comment fails on image and video posts. Pages also repeat IDs (`#search`, `#sort`, `#update-status`, `#feeds`/`#feed_list`, the Reddit popup), leave `href` unquoted, put `<legend>` outside a `<fieldset>`, and nest a `<button>` in an `<a>`.

#### Solution

- Load `highlighted.js` after the post-type chain when `single_thread`.
- Quote the two `href={{ post.media.url }}`.
- Rename the comment search input and form, drop the settings "Why?" details' ids and give the post-footer Reddit popup its own id (`#popup-post`) with class-based children, moving the matching CSS selectors.
- Settings list headings become `<h2>`; RSS buttons become small GET forms.

#### Notes

- **Location:** `templates/post.html`, `templates/utils.html`, `templates/settings.html`, `templates/subreddit.html`, `templates/user.html`, `static/style.css`

- **Verify:** `cargo test --locked html_markup_and_highlighted` passes.

- Boundary: rendered pages; the CSS moves were compared before and after in a browser.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**: `html_checks::assert_valid_markup(page, html)` reports duplicate ids, unquoted attribute values, `<legend>` outside `<fieldset>` and a `<button>` inside `<a>`.

- `html_markup_and_highlighted_*` per page module assert none; the subreddit and profile RSS tests run under `REDLIB_ENABLE_RSS=on` and check the form still asks for the viewed sort.
- `post.rs`: `html_markup_and_highlighted_script_loads_for_every_post_type_in_a_single_thread` (image, video, gallery, link, self) and `..._is_not_loaded_outside_a_single_thread`.

Red failures: `duplicate id "search"`, `duplicate id "popup"`, `unquoted href`, `legend outside fieldset`, `button inside a`, no `highlighted.js` on the image post.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Tasks 2, 6, 7

- **Supports:** M5.5

</details>

- [x] Task 8 complete

---

### Task 9: Integration / boundary verification

#### Problem

The milestone filters each prove one defect. The slice also has to leave the crate formatted, lint-clean and green, and the scripts and styles have to work in a browser.

#### Solution

Run the milestone filters, the script tests, the three repository gates and a Playwright pass, then record the results in Acceptance Evidence below.

#### Notes

- **Location:** repository root

- **Verify:** `node --test tests/js/`, `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` and `cargo test --locked` all exit 0.

- Boundary: offline suite plus a local browser. Live Reddit stays untouched.

<details>
<summary>⚙️ <b>Technical details</b></summary>

- `cargo test --locked opensearch_and_og`, `template_a11y_and_html`, `a11y_markup`, `html_markup_and_highlighted`, then the full suite.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Tasks 1–8

- **Supports:** M5.1, M5.2, M5.3, M5.4, M5.5

</details>

- [x] Task 9 complete

## Acceptance Evidence

Slice 5 implemented on 2026-11-28 against baseline `f81dcf4` (Slice 4 head).

- **Checks:** `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D warnings` clean; `cargo test --locked` passes 195 library tests (20 live-Reddit tests ignored), 30 binary tests, 3 `healthcheck` and 2 `startup_config` integration tests offline; `node --test tests/js/` passes 25 tests (after the review fixes).
- **Milestone filters (all `--locked`):** `opensearch_and_og` 11 lib + 5 bin; `template_a11y_and_html` 12; `a11y_markup` 9; `html_markup_and_highlighted` 11.
- **Decisions:** `to_absolute_url` already existed (RSS), so the plan's proposed signature is not added; `request_base_url` and `absolute_url` are new. `og:url` and `twitter:url` use the same base as the image tags, since they are relative in the same place. `error-446` is deleted rather than guarded: no template has the element. The `#update-status` fix is in the Task 2 commit. The body attribute and the script that reads it landed together (Task 6).
- **Scripts need no build step:** the tests load `static/*.js` into `node:vm` with fakes in `tests/js/support/dom.js`; the content hash the server puts in `?v=` follows the file. The command is also in the README under Building.

### Red-green

| Milestone | Test(s) | Observed red failure |
| --- | --- | --- |
| M5.1 | `playHLSVideo.test.js`: network, backoff, media, fallback, no-MP4 | `startLoad` called five times and `destroy` never; `video.src` stayed `about:blank`; no `.hls_error` message |
| M5.1 | `check_update.test.js`: outdated, missing commit | status was `⚠️ Error checking update status: TypeError: Cannot read properties of undefined (reading 'remove')` |
| M5.1 | `copy.test.js` (4 tests) | `TypeError: Cannot read properties of undefined (reading 'writeText')`; status stayed empty |
| M5.2 | `opensearch_and_og_descriptor_*` (3, bin) | body still held `https://localhost:8080/...`; `evil"><x` got `200` |
| M5.2 | `opensearch_and_og_request_base_url_*`, `..._absolute_url_only_prefixes_site_paths` | `left: None right: Some("http://redlib.example")`; `left: "/thumb/a/b.jpg" right: "https://redlib.example/thumb/a/b.jpg"` |
| M5.2 | `opensearch_and_og_meta_urls_are_absolute_when_the_base_is_known` | `left: Some("/thumb/b/xyz.jpg") right: Some("https://redlib.example/thumb/b/xyz.jpg")` |
| M5.3 | `template_a11y_and_html_*` (all page modules) | `<div#video_quality> is inside <head>`; `label for="post_sort" points at nothing` (also `fixed_navbar`, `hide_sidebar_and_summary`, `disable_visit_reddit_confirmation`); `label for="video_quality" points at <div#video_quality>`; `<button.submit> has no accessible name`; `<select#sort_options>`, `<select#timeframe>`, `<select#sort_select>`, `<select#commentSortSelect>`, `<input#encoded_prefs>` and the comment search input unnamed; `<p> opened 16 times, closed 15` (gallery, shown with a one-line revert) |
| M5.3 | `template_a11y_and_html_video_quality_travels_on_the_body`, `..._nav_icons_name_themselves` | `<body class="` had no `data-video-quality`; the Reddit icon had no `<title>` |
| M5.3 | `playHLSVideo.test.js` (every test, once the fake page offered the body attribute only) | `TypeError: document.getElementById is not a function` |
| M5.4 | `a11y_markup_*` (post, subreddit, home, wiki, wall, search, settings, profile, duplicates, error, info, NSFW landing) | `the first link in <body> is not a skip link to #main-content`; `expected one <main id="main-content">, found []`; `expected one <h1>, found 0` (search, settings) and `found 2`/`3` (the leave-Redlib popup's `<h1>`, once per popup) |
| M5.4 | `a11y_markup_spoiler_spans_can_take_keyboard_focus` | `<span class="md-spoiler-text">secret</span>` had no `tabindex` |
| M5.5 | `html_markup_and_highlighted_script_loads_for_every_post_type_in_a_single_thread` | `image thread: left: 0 right: 1` |
| M5.5 | `html_markup_and_highlighted_*_valid_markup` | `id="search" is used 2 times`; `id="sort"` 2; `id="popup"`, `id="goback"`, `id="toreddit"`, `id="reddit_url"` 2 on every post page; `id="feeds"` and `id="feed_list"` 2 on settings; `<legend> is outside a <fieldset>` twice; `<a[/vid/a/720.mp4]> has an unquoted href`; `id="update-status" is used 2 times` (error page, shown with a one-line revert) |
| M5.5 | `html_markup_and_highlighted_{subreddit,profile}_rss_button_is_not_inside_a_link` | `<a[/r/rust.rss?sort=top&t=week]> contains a <button>` |

The `html_checks` self-tests (`*_checker_*`, 4 tests) assert each report line on a defective snippet and accept a clean one, so the helper cannot pass by finding nothing.

### Browser verification (Playwright CLI, Edge)

Pages were rendered by the real templates from fixtures and served beside the real `/style.css` from a local `redlib` on a loopback port (Reddit returns 403 here). `/settings` and `/opensearch.xml` were checked on the running binary.

- **Skip link:** first Tab on a listing and on `/settings` focuses "Skip to content" (visible, top-left, with the focus ring); Enter then Tab lands on the first control inside `<main>` (the Theme select on `/settings`, the Hot tab on a listing), past the navbar.
- **Hidden `<h1>`:** before the stylesheet change it rendered as a large "r/rust" beside the tabs; after, the layout matches the earlier screenshot.
- **Tab rings:** the unselected "New" tab and the selected "Top" tab show a full ring inside the tab; with `outline-offset: 0` forced, the top and bottom of the ring were clipped (the audit's defect). The pagination ring is not clipped at offset 0, so it is unchanged. User-profile listing tabs match.
- **Mobile search:** at 400 px the search-page input reported `outline-style: none` before; after, it computes `solid 2px` and the ring is fully visible (the first fix left it clipped by the form's `overflow: auto`, hence the `#search_sort :focus-visible` rule). The navbar search ring is visible at 400 px.
- **Blurred posts:** focusing a blurred post's title link took its body (self post) or thumbnail from `blur(4px)`/`blur(4.8px)` to `none`.
- **Spoilers:** Tab from the post title reached the `tabindex="0"` spoiler span, its colour went from transparent to the text colour, and the next Tab hid it again.
- **OpenSearch:** the live `/opensearch.xml` gave `http://127.0.0.1:8081/...` for the image, search template and search form.
- **Accessibility tree:** the `/settings` snapshot names every combobox and checkbox, including `Default subreddit post sort:`.
- **Before/after render comparison (M5.5):** the settings page with subscriptions, filters and the "Why?" help open is pixel-identical to the earlier render (cookies set, same viewport) after the `<h2>` and `.prefs-group details` selector changes; the new headings needed `2px` side padding to match a legend. Listing, user and post pages looked the same. The Reddit popup's computed colour, background, width, padding, font size, border and weight match before and after the class renames.

### Corrections to the plan

- Plan: `pub fn to_absolute_url(path: &str) -> String` in `src/utils.rs`. Done instead: it already exists (RSS, `REDLIB_FULL_URL` only), so the new pieces are `request_base_url(&Request)` and `absolute_url(base, url)`.
- Plan: "Render `opensearch.xml` dynamically from `REDLIB_FULL_URL` or the request `Host`." Done as an Askama template with `Host` accepted only when it parses as a plain `host[:port]`; unusable hosts get `400`.
- Plan: "`check_update.js` guards `#error-446`". Done instead: the dead lines are deleted.
- Plan (M5.4): "CSS/template checks confirming `:focus-within` on `.post_blurred`/`.md-spoiler-text` ... no `#search:focus { outline: 0 }`". Not done as tests: the brief forbids assertions on stylesheet text. Focus ring, reveal and skip-link behaviour were checked in the browser (above).
- Plan (M5.5): the duplicate popup ids on post pages (`#popup`, `#goback`, `#toreddit`, `#reddit_url`) and the settings `#feeds`/`#feed_list` pair were not in the plan's list; the unique-id check found them and they are fixed.

### Not verified

- Real `hls.js` against a real stream: the retry, backoff and fallback logic is tested against a fake `Hls`; the `.hls_error` message styling was not seen in a browser.
- A screen reader announcing the Copy status, the HLS message or the skip link; the roles and attributes follow the `accessibility` and `copy-button` guides but no assistive technology was run.
- `execCommand('copy')` in a real browser over plain HTTP (the fake proves the call and the selection, not the browser's clipboard).
- Touch behaviour of the spoiler and blur reveal: the reveal follows `:focus-within`, which a tap sets, but no touch device was used.
- Post, user and subreddit pages with live Reddit data. The templates were rendered from fixtures only. The wiki's own `<h1>` (from Reddit's HTML) can add a second `<h1>` to a wiki page.
- A repeated Copy within two seconds shows the same text, which some screen readers do not announce again (the `copy-button` guide's VoiceOver note).
- The `.github` workflows do not run `node --test tests/js/`; wiring CI is Slice 7.

### Review

An adversarial reviewer read `f81dcf4..c602718` and ran the script and OpenSearch tests. No Critical findings. Verified findings and their dispositions:

| Finding | Disposition | Regression test |
| --- | --- | --- |
| `/opensearch.xml` varies on `X-Forwarded-Proto` but sent `Vary: Host` with `public, s-maxage=86400`, so a shared cache could keep one requester's scheme for a day | Fixed: without `REDLIB_FULL_URL` it is `private, max-age=300` with `Vary: Host, X-Forwarded-Proto`; the configured URL keeps public caching | `opensearch_and_og_descriptor_built_from_request_headers_is_not_shared_cacheable` (red: header was `public, max-age=1209600, s-maxage=86400`), `opensearch_and_og_descriptor_from_the_configured_url_is_publicly_cacheable` (control) |
| `Host` validation accepted `a:b`, `a:99999999`, `redlib.example:`, `*`, `a,b`, `a'b&c;d`, `a!b`, `redlib.example.` and did not lower-case | Fixed: `normalise_host` accepts a DNS name or IPv4 address (no empty or hyphen-edged label, so a trailing dot is rejected) or a bracketed IPv6 literal, with a port of 1 to 65535, and lower-cases it | `opensearch_and_og_request_base_url_rejects_hosts_that_are_not_host_and_port` (red: `left: Some("http://a:b") right: None`), `..._accepts_names_addresses_and_ports_in_lower_case` (red: `left: Some("http://REDLIB.Example")`) |
| No test showed `&` or `'` cannot reach the XML or HTML unescaped | Pinned: a base URL with `&`, `'`, `"` and `<` renders escaped in the descriptor and the `og:` tags. These pass before and after, because Askama already escapes; they guard it | `opensearch_and_og_configured_urls_cannot_inject_markup_into_the_descriptor`, `opensearch_and_og_meta_urls_escape_a_configured_base` |
| HLS retry counters never reset, so three fatal errors spread over a long video exhausted the budget; `giveUp` left a blocked `play()` promise unhandled | Fixed: `Hls.Events.FRAG_LOADED` resets both counters; `giveUp` catches the `play()` rejection | `a loaded fragment restores the retry budget, so sporadic errors over a long video do not add up` (red: `destroy` after the fourth error), `a blocked play() on the MP4 fallback is handled, not left as an unhandled rejection` (red: no handler attached), `errors with no fragment loaded in between still run out of budget` (control) |
| The `execCommand` fallback left focus on the readonly field | Fixed: focus returns to the element that had it | `the execCommand fallback gives focus back to the Copy button`, `focus returns to the Copy button when the fallback fails too` (both red: focus stayed on the field), `copying with the Clipboard API leaves focus alone` (control) |
| The HLS tests could not catch a dropped `if (!gaveUp)` guard, shared retry counters, or errors handled after giving up | Strengthened. I removed each guard in turn and each change failed a test | `a pending retry does not restart the stream once the player has given up`, `network and media errors each have their own budget of three`, `errors after giving up change nothing`, `non-fatal errors are ignored` |
| A proxy that rewrites `Host` reproduces the original #25 symptom, and the behaviour was undocumented | Documented in the README next to `FULL_URL`: set `REDLIB_FULL_URL`; `X-Forwarded-Host` and `Forwarded` are ignored on purpose | None (documentation) |

Accepted, not changed: the `node --test tests/js/` command is not in any CI workflow (Slice 7). The leave-Redlib popup's `<h2>` precedes the `<h1>` in the DOM but stays `visibility: hidden` until `:target`.
