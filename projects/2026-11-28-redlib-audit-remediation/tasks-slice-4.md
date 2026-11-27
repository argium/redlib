# Redlib Audit Remediation, Slice 4 — Task List

- **Status:** Complete
- **Author:** Argi
- **Created:** 2026-11-28
- **Language:** Rust
- **Source:** [plan.md](./plan.md), Slice 4 (S4): Domain Parsing, Feeds, Search, Emotes & Settings Versioning

## Summary

Several Reddit views quietly lose or corrupt what visitors wrote: the `/r/:sub/w/*page` shortcut drops the page, search terms with `&`, `#`, `+` or `%` are cut short, RSS ignores `REDLIB_ENABLE_RSS=off` and mangles escaping, comment search matches HTML tags, emotes swap images when a comment holds a GIF, and failed gallery items render empty images. These tasks fix each handler against offline JSON fixtures and give the settings export a real version header, so a later preference field no longer breaks every exported string.

## Starting Context

Existing code the tasks build on (verified against `d3b4b58`, the Slice 3 head; do not re-explore):

- `src/main.rs:348` — the `/r/:sub/w/*page` handler reads `r.param("wiki")`; `/w/*page` at `src/main.rs:363` already reads `page`. `build_app(None)` returns the real route table and the `serve_app()` / `fetch()` helpers in `src/main.rs` tests serve it on a loopback port.
- `src/instance_info.rs:169` — the raw `/info.txt` format labels `SFW only, Pushshift frontend, RSS enabled, Full URL, Remove default feeds` are followed by arguments in the order `sfw_only, enable_rss, full_url, default_remove_default_feeds, pushshift`. `InstanceInfo` derives `Default`; `to_string(&StringType)` is the renderer.
- `src/config.rs` — `Config` has no `default_autoplay_videos`. `get_setting_from_config` returns `None` for `REDLIB_DEFAULT_AUTOPLAY_VIDEOS`, so `utils::setting(req, "autoplay_videos")` never sees the server default. `app.json:41,44` name `REDLIB_USE_HLS` and `REDLIB_HIDE_HLS_NOTIFICATION`; the real keys are `REDLIB_DEFAULT_USE_HLS` and `REDLIB_DEFAULT_HIDE_HLS_NOTIFICATION`.
- `src/subreddit.rs:581`, `src/user.rs:134` — both `rss` handlers test `config::get_setting("REDLIB_ENABLE_RSS").is_none()` instead of `utils::enable_rss()`. `subreddit::rss` fetches without `raw_json=1` and `decode_html(&post.body).unwrap()`; `user::rss` fetches with `raw_json=1` but still runs `decode_html`, and sets no channel `<link>`. `Post::from_listing(&Value)` parses a listing without any network call.
- `src/search.rs:114,140,170` — `q: query.replace('"', "&quot;")`; `search_subreddits` only turns spaces into `+`. `templates/search.html:4,15,47,63,114,120` embed `params.q` unencoded (two with `|safe`).
- `src/post.rs:154` — `query_comments` tests the rendered `c.body` (HTML). `build_comment` takes `&Preferences` since Slice 3.
- `src/utils.rs:1340` — `rewrite_emotes` zips `$[*].id` with `$[*].s.u` into parallel vectors and takes the first `s.y` for every emote. `src/utils.rs:292` — `GalleryMedia::parse` emits an item for every `gallery_data.items` entry, valid or not.
- `src/utils.rs:631` — `Preferences` is `#[revisioned(revision = 1)]` but `to_bincode`/`settings::encoded_restore` use plain `bincode`. `KNOWN_GOOD_CONFIGS` at `src/utils.rs:2051` holds three real v1 exports (base2048 of deflate of bincode).
- `templates/utils.html:93,252`, `templates/duplicates.html:78` — flair search links put `post.flair.text` into the query string raw. `templates/wall.html:8` (`?redir={{ url }}`) and `templates/nsfwlanding.html:24` (`&redirect={{url}}`) do the same with the request URI.

Conventions and gotchas:

- Tabs (`rustfmt.toml`). Windows builds need `cmake` and `nasm` on `PATH` (see [plan.md Prerequisites](./plan.md#prerequisites)).
- Tests never touch live Reddit. RSS handlers cannot run offline past their gate, so the feed bodies come from pure builders fed with `Post::from_listing` fixtures.
- Tests that read `CONFIG` run under `sealed_test` (one process each) because `CONFIG` is a `LazyLock`.
- No new crates. `percent-encoding` is already a dependency, and Askama's `urlencode` feature (off because the crate uses `default-features = false`) switches on the `urlencode_strict` filter that uses it.
- The plan's `SearchParams.q_enc` is refined away: templates apply `|urlencode_strict` to the raw `q` instead, so no second field can drift out of step.
- #48's HTML-markup half (quoted `href`, duplicate IDs, `<legend>`, `<button>` in `<a>`) belongs to Slice 5 and is not touched.

## Tasks

### Task 1: The wiki shortcut keeps its page, `/info.txt` lines up, and the autoplay default reaches `Config`

#### Problem

`/r/rust/w/faq/index` redirects to `/r/rust/wiki/` because the handler reads a param named `wiki` that the route never defines. `/info.txt` prints `Pushshift frontend` with the RSS value, `RSS enabled` with the full URL and so on, because the format arguments are in a different order from the labels. `REDLIB_DEFAULT_AUTOPLAY_VIDEOS` is documented in the README and `.env.example` but `Config` never reads it, and `app.json` offers Heroku users two variable names the server ignores.

#### Solution

- The `/r/:sub/w/*page` handler reads `r.param("page")`.
- The `/info.txt` arguments follow the label order; the new autoplay default is listed in both the text and HTML views.
- `Config.default_autoplay_videos` (`REDLIB_DEFAULT_AUTOPLAY_VIDEOS`, legacy `LIBREDDIT_` alias) is parsed in `try_load` and answered by `get_setting_from_config`, which is how `Preferences::new` picks up server defaults.
- `app.json` names `REDLIB_DEFAULT_USE_HLS` and `REDLIB_DEFAULT_HIDE_HLS_NOTIFICATION` and gains `REDLIB_DEFAULT_AUTOPLAY_VIDEOS`.

#### Notes

- **Location:** `src/main.rs`, `src/instance_info.rs`, `src/config.rs`, `app.json`

- **Verify:** `cargo test --locked wiki_info_and_config` passes when the route, `/info.txt`, `Preferences` default and `app.json` tests pass.

- Boundary: the real route table on a loopback port; `InstanceInfo` rendered from a hand-built `Config`; `Preferences::new` on a cookie-less request. No Reddit.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**:

- `src/main.rs`: `wiki_info_and_config_wiki_shortcut_keeps_the_page_path` — `GET /r/rust/w/faq/index` answers `302` with `Location: /r/rust/wiki/faq/index`; `GET /r/rust/w` still goes to `/r/rust/wiki`.
- `src/instance_info.rs`: `wiki_info_and_config_info_txt_values_sit_under_their_labels` — an `InstanceInfo` whose config gives every field a distinct value; each `Label: value` line of the raw text names its own field's value.
- `src/config.rs`: `wiki_info_and_config_autoplay_default_is_applied_to_preferences` (`sealed_test`, env `REDLIB_DEFAULT_AUTOPLAY_VIDEOS=on`, no cookie: `Preferences::new(..).autoplay_videos == "on"`) and `wiki_info_and_config_app_json_env_names_are_config_keys` (every `env` key of `app.json` is a serialised `Config` key).

Red failures: `/r/rust/wiki/` vs `/r/rust/wiki/faq/index`; `Pushshift frontend` carrying the RSS value; `"" vs "on"`; `REDLIB_USE_HLS is not a Config key`.

**Implementation guidance (green)**:

- Follow `/w/*page` in `src/main.rs:363`.
- Move `self.config.pushshift` after `sfw_only` in `info_txt`'s arguments; add `Default autoplay videos` after `Default use HLS` in both views.
- Copy the `default_use_hls` pattern for the field, `try_load` and `get_setting_from_config`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.1

</details>

- [x] Task 1 complete

---

### Task 2: RSS respects `REDLIB_ENABLE_RSS`, keeps one level of escaping and gives user feeds a `<link>`

#### Problem

`REDLIB_ENABLE_RSS=off` still serves feeds, because the handlers only check that the variable is set. The subreddit feed fetches without `raw_json=1`, so titles reach the XML writer already entity-escaped and come out as `&amp;amp;`, and a post whose body fails to decode panics the handler. The user feed does fetch raw JSON but still decodes the HTML body, which turns text a visitor typed as `&lt;img ...&gt;` into live markup. It also omits the channel `<link>` that RSS 2.0 requires.

#### Solution

- Both handlers refuse with `utils::enable_rss()`.
- Both fetch with `raw_json=1` and stop calling `decode_html` (feed bodies are `rewrite_urls` of the HTML Reddit sent; the `rss` crate escapes it once).
- The feed bodies come from `subreddit::feed_xml` and `user::feed_xml`, pure functions over parsed `Post`s, so tests need no network. The user channel carries `<link>{REDLIB_FULL_URL}/user/{name}</link>`, mirroring the subreddit feed.

#### Notes

- **Location:** `src/subreddit.rs`, `src/user.rs`

- **Verify:** `cargo test --locked rss_feeds` passes when the refusal and feed content tests pass.

- Boundary: the handlers with `REDLIB_ENABLE_RSS=off` (they must answer before any fetch); feeds built from fixture listings and read back with `rss::Channel::read_from`. The fetch-and-gate order for `on` needs live Reddit and is not run.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**, in `src/subreddit.rs` and `src/user.rs`:

- `rss_feeds_subreddit_off_refuses_before_fetching` and `rss_feeds_user_off_refuses_before_fetching` (`sealed_test`, `REDLIB_ENABLE_RSS=off`): the handler answers "RSS is disabled" within 5 s, on a thread of its own (`test_support::answer_without_fetching`). A handler that goes on to fetch blocks inside `OAUTH_CLIENT`'s initialiser, so a timeout on the same thread cannot fire.
- `rss_feeds_subreddit_listing_is_fetched_as_raw_json`: `feed_path("rust", "top", "t=week")` is `/r/rust/top.json?t=week&raw_json=1`.
- `rss_feeds_subreddit_feed_keeps_text_as_the_visitor_wrote_it` and `rss_feeds_user_feed_has_a_link_and_keeps_typed_text_as_text`: a fixture post titled `Q&A <b>"x"</b>` with body `&lt;img src=x&gt; &amp; left` round-trips through `rss::Channel::read_from` with the identical title, the body still holds `&lt;img src=x&gt;` and no `<img`, and the channel link is the one passed in.

Red failures: timeout (the handler went on to fetch, run with a dead proxy so nothing left the machine); `/r/rust/top.json?t=week` without `raw_json`; `<img src=x>` present in the content; empty channel link.

**Implementation guidance (green)**:

- Handler: `if !utils::enable_rss() { return Ok(error(..)) }`; `feed_path` appends `&raw_json=1` to the subreddit path.
- Drop `decode_html` from both feed builders and from `get_rss_image` (URLs from raw JSON need no decoding); remove the now-unused imports.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.2

</details>

- [x] Task 2 complete

---

### Task 3: Search terms and comment search survive `&`, `#`, `+` and `%`

#### Problem

Searching `C++ & AT&T #1 100%` loses everything after the first `&` when Redlib looks up subreddits, because that call only swaps spaces for `+`. The result page then puts the decoded query into pagination and "back" links unencoded, so NEXT searches for `C` and the title shows `&quot;` for every quote. Separately, searching inside a thread matches the comment's rendered HTML, so `href`, `class` or `md` hit nearly every comment.

#### Solution

- `search_subreddits` builds its path with a percent-encoded query (`subreddit_search_path`).
- `SearchParams.q` stays raw; `search.html` HTML-escapes it where shown and applies `|urlencode_strict` inside every `href`. The manual `&quot;` replacement goes.
- `query_comments` matches the comment's raw `body` and builds only the comments that match.

#### Notes

- **Location:** `src/search.rs`, `templates/search.html`, `src/post.rs`, `Cargo.toml` (Askama `urlencode` feature)

- **Verify:** `cargo test --locked search_encoding_and_comments` passes.

- Boundary: `SearchTemplate` rendered from a fixture, `subreddit_search_path`, and `query_comments` on a fixture comment tree. The `find` handler itself needs live Reddit.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**:

- `src/search.rs`: `search_encoding_and_comments_subreddit_lookup_encodes_the_query` — `subreddit_search_path("C++ & AT&T #1 100%", "")` is `/subreddits/search.json?q=C%2B%2B%20%26%20AT%26T%20%231%20100%25&limit=3`.
- `src/search.rs`: `search_encoding_and_comments_params_keep_the_query_as_raw_text` — `search_params` (one builder for both result branches) keeps `q` as typed.
- `src/search.rs`: `search_encoding_and_comments_links_carry_the_encoded_query` — render with `q = C++ & AT&T #1 100% "x"`: every `?q=` href (back, more, PREV, NEXT) holds the encoded text.
- `src/search.rs`: `search_encoding_and_comments_query_is_html_escaped_once_where_shown` — the input `value` and `<title>` hold the query escaped once, and nothing contains `&quot;`.
- `src/post.rs`: `search_encoding_and_comments_matches_text_not_markup` — a link-only comment is not found by `href`, `class`, `md` or `div`; a plain comment is found by its words, in any letter case; a removed comment is not found by its archive link.

Red failures: `/subreddits/search.json?q=C+++&+AT&T+#1+...`; `say &quot;hi&quot;`; zero encoded links; the search box broken by the raw `"`; `href` matched the link comment.

**Implementation guidance (green)**:

- `Cargo.toml`: add `"urlencode"` to askama's features (no new crate; `percent-encoding` is already locked). `sort`, `t` and `restrict_sr` are encoded in the same links; cursor ids are Reddit-issued and left alone.
- Encode with `percent_encoding::utf8_percent_encode(q, NON_ALPHANUMERIC)`, so Rust and templates use one spelling (`%20`, never `+`, for a space).
- `query_comments`: compute `val(comment, "body").to_lowercase().contains(..)` before `build_comment`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.3

</details>

- [x] Task 3 complete

---

### Task 4: Emotes keep their own ID, link and size

#### Problem

`media_metadata` holds one entry per attachment in a comment: emotes (with `s.u` and `s.y`), but also GIFs and uploaded images that carry an `id` and no emote URL. `rewrite_emotes` collects all IDs and all links into two lists and pairs them by position, so one GIF shifts every later emote onto the wrong picture. Every emote also takes the size of the first entry.

#### Solution

`rewrite_emotes` walks the `media_metadata` entries and handles each as a unit: its own `id`, `s.u` and `s.y`. Entries that are not emotes (no emote ID, or an `s.u` outside Reddit's emote bucket) are skipped. `serde_json_path` was used only here, so the dependency is removed with it.

#### Notes

- **Location:** `src/utils.rs`

- **Verify:** `cargo test --locked rewrite_emotes` passes.

- Boundary: pure function on a JSON fixture.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Test (red)** — `src/utils.rs`, `rewrite_emotes_pairs_each_emote_with_its_own_link_and_size`: `media_metadata` (ordered by key, as `serde_json` keeps it) with an animated GIF whose key sorts first (it has `id` and `s.y` but no `s.u`), emote 2028 (size 60) and emote 2029 (size 20); the comment holds `:2028: and :2029: and :99:`. Each emote gets its own file and size, and `:99:` stays text. The existing `test_rewriting_emotes` single-emote case is the control that passes before and after.

Red failure: `:2028:` was replaced with `Second.png` at size 300 (the GIF's), and `:2029:` was left as text.

**Implementation guidance (green)**:

- `media_metadata.as_object().into_iter().flatten()`; read `entry["id"]`, `entry["s"]["u"]`, `entry["s"]["y"]` as `str`/`i64`.
- The two regexes now match the unquoted strings (`^...$`). The old code matched JSON-quoted strings and relied on the capture including the closing quote, so the format string now writes that quote itself.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.4

</details>

- [x] Task 4 complete

---

### Task 5: Flair-search and wall/NSFW redirect links are percent-encoded

#### Problem

A flair such as `C# & 100%` becomes `?q=flair_name%3A%22C# &amp; 100%%22`, which the browser cuts at `#`. The quarantine wall and the NSFW landing page put the request URI into a `redir=`/`redirect=` parameter unencoded, so a page with its own query string (`?sort=new&t=week`) comes back to the visitor with the tail dropped.

#### Solution

`templates/utils.html` (two flair links), `templates/duplicates.html`, `templates/wall.html` and `templates/nsfwlanding.html` apply `|urlencode_strict` to the interpolated value. The filter's output holds only unreserved characters and `%XX`, so HTML escaping cannot change it.

#### Notes

- **Location:** `templates/utils.html`, `templates/duplicates.html`, `templates/wall.html`, `templates/nsfwlanding.html`

- **Verify:** `cargo test --locked flair_gallery_and_prefs_versioning_links` passes.

- Boundary: templates rendered from fixtures (`SubredditTemplate`, `PostTemplate`, `DuplicatesTemplate`, the wall and the NSFW landing page). `subreddit.rs`/`user.rs` `redirect_url` is out of this finding's list and unchanged.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `flair_gallery_and_prefs_versioning_links_*`:

- `..._listing_flair_search_encodes_the_flair`, `..._post_flair_search_encodes_the_flair`, `..._duplicates_flair_search_encodes_the_flair`: the page for a post with flair `C# & 100%` links `/r/test/search?q=flair_name%3A%22C%23%20%26%20100%25%22&restrict_sr=on` (listing, post header, original post and duplicate on the duplicates page).
- `..._wall_form_returns_to_the_full_url`: submitting the form of `quarantine(/r/edgy?sort=new&t=week)` to `add_quarantine_exception` redirects to `/r/edgy?sort=new&t=week`.
- `..._nsfw_bypass_returns_to_the_full_url`: following the NSFW landing page's bypass link through `settings::update` redirects to the original post URL with its query.

Red failures: `q=flair_name%3A%22C# &#38; 100%%22`; `/r/edgy?sort=new` (tail lost at the first `&`).

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 3 (Askama `urlencode` feature)

- **Supports:** M4.5

</details>

- [x] Task 5 complete

---

### Task 6: Invalid gallery items are skipped

#### Problem

Reddit marks media that failed processing with `status` other than `valid` and no `s` block. `GalleryMedia::parse` still emits an item for it, and the gallery renders an `<img>`/`<a>` with an empty URL.

#### Solution

`GalleryMedia::parse` keeps an item only when its `media_metadata` entry has `status == "valid"` and a non-empty image URL (`s.u`, or `s.gif` for GIFs).

#### Notes

- **Location:** `src/utils.rs`

- **Verify:** `cargo test --locked flair_gallery_and_prefs_versioning_gallery` passes.

- Boundary: `Post::from_listing` on a gallery fixture.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Test (red)** — `flair_gallery_and_prefs_versioning_gallery_skips_invalid_items`: a gallery post with four items (valid image, `status: "failed"`, an id with no metadata, a valid GIF that uses `s.gif`) parses to the image and the GIF only.

Red failure: the failed and unknown items came back with an empty URL (`("failed upload", "")`, `("no metadata", "")`).

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.5

</details>

- [x] Task 6 complete

---

### Task 7: The settings export carries a version and reads old exports

#### Problem

`Preferences` is marked `#[revisioned(revision = 1)]`, but the export string is plain `bincode`, so the attribute does nothing. Adding a field changes the byte layout, and every export a visitor saved before the upgrade then fails to restore.

#### Solution

- Exports are `base2048(deflate(b"RLP" + version byte + bincode))`, version `2`. A v1 export (bare bincode) starts with the 8-byte length of `theme`, whose second byte is always `0`, so it can never start with `RLP`.
- `Preferences::from_export_string(encoded, limit)` owns the whole read path (base2048, bounded inflate, header dispatch): `RLP` + `2` reads the current layout, a bare payload reads as v1, any other version is an error.
- `to_export_string` replaces `to_bincode_str`; `available_themes` is `serde(skip)`, so it is not exported and a restored value is always empty.
- The `revision` crate's reader is not used: `String::deserialize_revisioned` allocates `vec![0; len]` from the length in the input (revision 0.17.1), so a 1 MiB import could abort the process. `bincode` refuses a length longer than the remaining input. The unused `#[revisioned]` attributes and the `revision` dependency are removed.
- `settings::encoded_restore` and `templates/settings.html` use the new names.

#### Notes

- **Location:** `src/utils.rs`, `src/settings.rs`, `templates/settings.html`, `Cargo.toml`

- **Verify:** `cargo test --locked flair_gallery_and_prefs_versioning_prefs` passes.

- Boundary: byte-level encode/decode, and `encoded_restore` on a request body. The three `KNOWN_GOOD_CONFIGS` v1 strings are decoded through the new reader.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `flair_gallery_and_prefs_versioning_prefs_*`:

- `..._export_round_trips_with_a_version_header`: customised preferences survive `to_export_string` then `from_export_string`, and the payload starts with `RLP` and version byte 2.
- `..._known_good_v1_exports_still_decode` (replaces the two `KNOWN_GOOD_CONFIGS` tests and the bincode loop test, which called `bincode` directly): every fixture decodes and re-exports to something that decodes to the same preferences.
- `..._malformed_exports_are_errors`: empty, header-only, truncated and huge-length payloads, text that is not base2048, and version 99 all return `Err` (the last names the version).
- `settings::tests::..._encoded_restore_accepts_v1_and_current_exports`: `encoded_restore` redirects for a payload made the old way (deflated bincode) and for a fresh export, both carrying `theme=dark`.

Red failures: `an export names its version`; `Failed to deserialize bytes into Preferences struct: io error:` (version 99 read as bare bincode). The v1 compatibility tests pass before and after: they guard the old format.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.5

</details>

- [x] Task 7 complete

---

### Task 8: Integration / boundary verification

#### Problem

The milestone filters each prove one defect. The slice also has to leave the whole crate formatted, lint-clean and green.

#### Solution

Run the four milestone filters and the three repository gates, then record the results in Acceptance Evidence below.

#### Notes

- **Location:** repository root

- **Verify:** `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` and `cargo test --locked` all exit 0.

- Boundary: offline suite. Live Reddit tests stay `#[ignore]`d; the RSS, search and `find` handlers' network paths are not exercised.

<details>
<summary>⚙️ <b>Technical details</b></summary>

- `cargo test --locked wiki_info_and_config`, `rss_feeds`, `search_encoding_and_comments`, `rewrite_emotes`, `flair_gallery_and_prefs_versioning`, then the full suite.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Tasks 1–7

- **Supports:** M4.1, M4.2, M4.3, M4.4, M4.5

</details>

- [x] Task 8 complete

## Acceptance Evidence

Slice 4 implemented on 2026-11-28 against baseline `d3b4b58` (Slice 3 head).

- **Checks:** `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D warnings` clean; `cargo test --locked` passes 150 library tests (20 live-Reddit tests ignored), 25 binary tests, 3 `healthcheck` and 2 `startup_config` integration tests offline.
- **Milestone filters (all `--locked`):** `wiki_info_and_config` 4 lib + 1 bin (the `sealed_test` case is counted twice by the runner); `rss_feeds` 7 (5 tests, 2 of them `sealed_test`); `search_encoding_and_comments` 5; `rewrite_emotes` 1 (plus the existing `test_rewriting_emotes` control); `flair_gallery_and_prefs_versioning` 10.
- **Decisions:** `SearchParams.q_enc` from the plan is not added: templates apply Askama's `urlencode_strict` to the raw `q` (Askama's `urlencode` feature, no new crate). The export header is `RLP` plus a version byte, with bincode inside; the `revision` crate is not used for reading (see Task 7) and is removed together with the dead `#[revisioned]` attributes. `serde_json_path` is removed because `rewrite_emotes` was its only user. `GalleryMedia::parse` keeps only `status == "valid"` items with a non-empty URL.

### Red-green

| Milestone | Test(s) | Observed red failure |
| --- | --- | --- |
| M4.1 | `wiki_info_and_config_wiki_shortcut_keeps_the_page_path` | `left: "/r/rust/wiki/" right: "/r/rust/wiki/faq/index"` |
| M4.1 | `wiki_info_and_config_info_txt_values_sit_under_their_labels` | `Pushshift frontend: left: Some("value-of-enable_rss") right: Some("value-of-pushshift")` |
| M4.1 | `wiki_info_and_config_autoplay_default_is_applied_to_preferences` | `left: "" right: "on"` |
| M4.1 | `wiki_info_and_config_app_json_env_names_are_config_keys` | `app.json names variables that Config does not read: ["REDLIB_HIDE_HLS_NOTIFICATION", "REDLIB_USE_HLS"]` |
| M4.2 | `rss_feeds_subreddit_off_refuses_before_fetching`, `rss_feeds_user_off_refuses_before_fetching` | `the handler must answer without asking Reddit: Timeout` (run with a dead proxy; an earlier attempt with a timeout on the handler's own thread hung for 25 minutes because the fetch blocks in `OAUTH_CLIENT`'s initialiser) |
| M4.2 | `rss_feeds_subreddit_listing_is_fetched_as_raw_json` | `left: "/r/rust/top.json?t=week" right: "/r/rust/top.json?t=week&raw_json=1"` |
| M4.2 | `rss_feeds_subreddit_feed_keeps_text_as_the_visitor_wrote_it` | content was `<div class="md"><p>I typed <img src=x> & left</p></div>` |
| M4.2 | `rss_feeds_user_feed_has_a_link_and_keeps_typed_text_as_text` | `left: "" right: "https://redlib.example/user/spez"` |
| M4.3 | `search_encoding_and_comments_subreddit_lookup_encodes_the_query` | `/subreddits/search.json?q=C+++&+AT&T+#1+100%+"x"&limit=3` |
| M4.3 | `search_encoding_and_comments_params_keep_the_query_as_raw_text` | `left: "say &quot;hi&quot;"` |
| M4.3 | `search_encoding_and_comments_links_carry_the_encoded_query` | `all: every link carries the encoded query: left: 0 right: 3` |
| M4.3 | `search_encoding_and_comments_query_is_html_escaped_once_where_shown` | `search box` (the raw `"` ended the `value` attribute) |
| M4.3 | `search_encoding_and_comments_matches_text_not_markup` | `href is markup, not comment text: left: ["link"] right: []` |
| M4.4 | `rewrite_emotes_pairs_each_emote_with_its_own_link_and_size` | first emote rendered as `Second.png` at `width="300"`, `:2029:` left as text |
| M4.5 | `flair_gallery_and_prefs_versioning_links_{listing,post,duplicates}_flair_search_encodes_the_flair` | `q=flair_name%3A%22C# &#38; 100%%22&restrict_sr=on` |
| M4.5 | `flair_gallery_and_prefs_versioning_links_wall_form_returns_to_the_full_url` | `left: "/r/edgy?sort=new" right: "/r/edgy?sort=new&t=week"` |
| M4.5 | `flair_gallery_and_prefs_versioning_links_nsfw_bypass_returns_to_the_full_url` | `left: "/r/rust/comments/abc/post/?sort=new"`, tail after `&` lost |
| M4.5 | `flair_gallery_and_prefs_versioning_gallery_skips_invalid_items` | items `("failed upload", "")` and `("no metadata", "")` present |
| M4.5 | `flair_gallery_and_prefs_versioning_prefs_export_round_trips_with_a_version_header` | `an export names its version` |
| M4.5 | `flair_gallery_and_prefs_versioning_prefs_malformed_exports_are_errors` | `Failed to deserialize bytes into Preferences struct: io error:` (version 99 accepted as bare bincode) |
| M4.5 | `..._prefs_known_good_v1_exports_still_decode`, `..._prefs_encoded_restore_accepts_v1_and_current_exports` | pass before and after by design: they pin the v1 format and `KNOWN_GOOD_CONFIGS` |

### Corrections to the plan

- Plan: `SearchParams.q_enc`. Done instead: `|urlencode_strict` in the templates.
- Plan: `Preferences` "versioned serialisation" via `revision`. Done instead: explicit `RLP` + version byte header with bincode (research #49 allows either). `revision` 0.17.1 sizes buffers from lengths in its input, which is unsafe for a user-supplied import string.
- Finding numbering: the task brief lists the settings versioning as #50; in `research.md` it is #49 and the gallery fix is #50. Both are done.
- `rewrite_emotes` also fixed a latent oddity: the old pattern captured the JSON quote that closed `src`; the new code writes that quote itself.

### Not verified

- The live paths of `subreddit::rss`, `user::rss` (with `REDLIB_ENABLE_RSS=on`), `search::find` and `post::item` need live Reddit. Their pure parts (feed bodies, `feed_path`, `search_params`, `subreddit_search_path`, `query_comments`) are tested.
- `/r/:sub/w/*page` is tested through the real route table, but only the redirect, not the wiki page it leads to.
- `redirect_url` in `subreddit.rs` and `user.rs` (built with `replace`) and the comment-search query parsing in `post::item` (`COMMENT_SEARCH_CAPTURE`, `%20` and `+` only) were not in this finding's list and are unchanged. A `%26` or `%2B` already in the page URL still reaches those redirect links decoded.
- The settings page's export box and the browser's use of it were not exercised in a browser.
- No browser check of the encoded links; they were verified by round trips through the real handlers (`add_quarantine_exception`, `settings::update`) instead.
