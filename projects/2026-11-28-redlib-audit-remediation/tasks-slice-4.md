# Redlib Audit Remediation, Slice 4 — Task List

- **Status:** Ready
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

- [ ] Task 1 complete

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

- `rss_feeds_off_refuses_before_fetching` (`sealed_test`, `REDLIB_ENABLE_RSS=off`, 5 s timeout): `rss(req)` answers with "RSS is disabled". Red: the call goes on to fetch from Reddit (the red run sets a dead proxy so nothing leaves the machine).
- `rss_feeds_subreddit_titles_are_escaped_once`: a post titled `Q&A <b>"x"</b>` round-trips through `Channel::read_from` as the identical title; the link is the one passed in.
- `rss_feeds_subreddit_body_is_not_decoded_into_markup` and `rss_feeds_user_body_is_not_decoded_into_markup`: a body containing `&lt;img src=x&gt;` still holds `&lt;img` after the XML round trip and holds no `<img`.
- `rss_feeds_user_channel_has_a_link`: `channel.link()` is `https://redlib.example/user/spez`.

Red failures: reaching Reddit; `Q&amp;A` vs `Q&A`; `<img src=x>` present; empty link.

**Implementation guidance (green)**:

- Handler: `if !utils::enable_rss() { return Ok(error(..)) }`; append `&raw_json=1` to the subreddit path.
- Drop `decode_html` from both feed builders and from `get_rss_image` (URLs from raw JSON need no decoding); remove the now-unused imports.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.2

</details>

- [ ] Task 2 complete

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
- `src/search.rs`: `search_encoding_and_comments_links_carry_the_encoded_query` — render with `q = C++ & AT&T #1 100% "x"`: every `?q=` href (back, more, PREV, NEXT) holds the encoded text, the input `value` and `<title>` hold it HTML-escaped once, and nothing contains `&quot;`.
- `src/post.rs`: `search_encoding_and_comments_matches_text_not_markup` — a link-only comment is not found by `href`, `class` or `md`; a plain comment is found by its words, and by a different letter case; a removed comment is not found by its archive link.

Red failures: `?q=C++ & AT&T`; `&quot;` in the page; `href` matched 2 comments.

**Implementation guidance (green)**:

- `Cargo.toml`: add `"urlencode"` to askama's features (no new crate; `percent-encoding` is already locked).
- Encode with `percent_encoding::utf8_percent_encode(q, NON_ALPHANUMERIC)`, so Rust and templates use one spelling (`%20`, never `+`, for a space).
- `query_comments`: compute `val(comment, "body").to_lowercase().contains(..)` before `build_comment`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.3

</details>

- [ ] Task 3 complete

---

### Task 4: Emotes keep their own ID, link and size

#### Problem

`media_metadata` holds one entry per attachment in a comment: emotes (with `s.u` and `s.y`), but also GIFs and uploaded images that carry an `id` and no emote URL. `rewrite_emotes` collects all IDs and all links into two lists and pairs them by position, so one GIF shifts every later emote onto the wrong picture. Every emote also takes the size of the first entry.

#### Solution

`rewrite_emotes` walks the `media_metadata` entries and handles each as a unit: its own `id`, `s.u` and `s.y`. Entries that are not emotes (no emote ID or an `s.u` outside the emote bucket) are skipped, and the `serde_json_path` dependency use goes with the parallel vectors.

#### Notes

- **Location:** `src/utils.rs`

- **Verify:** `cargo test --locked rewrite_emotes` passes.

- Boundary: pure function on a JSON fixture.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Test (red)** — `src/utils.rs`, `rewrite_emotes_pairs_each_emote_with_its_own_link_and_size`: `media_metadata` with, in order, a `giphy|…` GIF entry (no `s.u`), emote `:1:` (size 20) and emote `:2:` (size 60); the comment text holds `:1: :2:`; the output has the first emote's URL at 20 and the second's at 60, in that order. A second case, `rewrite_emotes_leaves_unknown_codes_alone`, keeps `:99:` text when no entry has that id.

Red failure: the first emote shows the second emote's URL and size 20 for both.

**Implementation guidance (green)**:

- `media_metadata.as_object().into_iter().flatten()`; read `entry["id"]`, `entry["s"]["u"]`, `entry["s"]["y"]` as `str`/`i64`.
- Keep `REDDIT_EMOTE_ID_NUMBER_REGEX` and `REDDIT_EMOTE_LINK_REGEX` anchored on the unquoted strings; the old code matched JSON-quoted strings and relied on the trailing `"` being captured, so the new format string closes the `src` quote itself.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.4

</details>

- [ ] Task 4 complete

---

### Task 5: Flair-search and wall/NSFW redirect links are percent-encoded

#### Problem

A flair such as `C# & 100%` becomes `?q=flair_name%3A%22C# &amp; 100%%22`, which the browser cuts at `#`. The quarantine wall and the NSFW landing page put the request URI into a `redir=`/`redirect=` parameter unencoded, so a page with its own query string (`?sort=new&t=week`) comes back to the visitor with the tail dropped.

#### Solution

`templates/utils.html` (two flair links), `templates/duplicates.html`, `templates/wall.html` and `templates/nsfwlanding.html` apply `|urlencode_strict` to the interpolated value. Askama still HTML-escapes nothing twice: the filter's output holds only unreserved characters and `%XX`.

#### Notes

- **Location:** `templates/utils.html`, `templates/duplicates.html`, `templates/wall.html`, `templates/nsfwlanding.html`

- **Verify:** `cargo test --locked flair_gallery_and_prefs_versioning_links` passes.

- Boundary: templates rendered from fixtures (`post_in_list` via `SubredditTemplate`, `WallTemplate`, `NSFWLandingTemplate`). `subreddit.rs`/`user.rs` `redirect_url` is out of this finding's list and unchanged.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `flair_gallery_and_prefs_versioning_links_*`:

- `..._flair_search_link_encodes_the_flair`: a listing post with flair `C# & 100%` renders `q=flair_name%3A%22C%23%20%26%20100%25%22&restrict_sr=on` and no raw `C# &`.
- `..._wall_redirect_keeps_the_query`: `quarantine(req for /r/edgy?sort=new&t=week)` renders `redir=%2Fr%2Fedgy%3Fsort%3Dnew%26t%3Dweek`; decoding that parameter gives the original path.
- `..._nsfw_landing_redirect_keeps_the_query`: the same for `redirect=` on the post landing page.

Red failures: raw `C# &amp; 100%` in the href; `redir=/r/edgy?sort=new&t=week`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 3 (Askama `urlencode` feature)

- **Supports:** M4.5

</details>

- [ ] Task 5 complete

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

**Test (red)** — `flair_gallery_and_prefs_versioning_gallery_skips_invalid_items`: a gallery post with three items (valid image, `status: "failed"`, valid id missing from metadata) parses to one `GalleryMedia`, whose URL is the valid one; a valid GIF entry uses `s.gif`.

Red failure: `left: 3 right: 1`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.5

</details>

- [ ] Task 6 complete

---

### Task 7: The settings export carries a version and reads old exports

#### Problem

`Preferences` is marked `#[revisioned(revision = 1)]`, but the export string is plain `bincode`. Adding a field changes the byte layout, so every export a visitor saved before the upgrade fails to restore. The `revision` attribute does nothing today.

#### Solution

- Exports are `base2048(deflate(b"RLPR" + revisioned bytes))`. `RLPR` is a magic header: a v1 bincode payload starts with the 8-byte length of `theme`, whose second byte is always `0`, so the header cannot be mistaken for it.
- `Preferences::from_export_bytes` accepts both: the header selects the revisioned reader; anything else is read as v1 bincode.
- `to_export_string` replaces `to_bincode_str`; `available_themes` is not exported (the restore handler already discards it).
- `settings::encoded_restore` and the settings page use the new names.

#### Notes

- **Location:** `src/utils.rs`, `src/settings.rs`, `templates/settings.html`

- **Verify:** `cargo test --locked flair_gallery_and_prefs_versioning_prefs` passes.

- Boundary: byte-level encode/decode and `encoded_restore` on a request body. The three `KNOWN_GOOD_CONFIGS` v1 strings are decoded through the new reader.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `flair_gallery_and_prefs_versioning_prefs_*`:

- `..._export_round_trips_and_is_versioned`: non-default preferences survive `to_export_string` → `from_export_bytes`, and the bytes start with `RLPR`.
- `..._known_good_v1_exports_still_decode` (replaces the two `KNOWN_GOOD_CONFIGS` tests that called `bincode` directly): every fixture decodes through `from_export_bytes` and re-exports to something that decodes to the same preferences.
- `..._encoded_restore_accepts_v1_and_current_exports`: `settings::encoded_restore` redirects for a `KNOWN_GOOD_CONFIGS` string and for a fresh export, to the same `/settings/restore/?…` shape.
- `..._unknown_bytes_are_an_error`: truncated and empty inputs return `Err`, not a panic.

Red failures: compile errors (new names) resolved by stubs that keep `bincode`, then `bytes do not start with RLPR`.

**Implementation guidance (green)**:

- `revision::Revisioned::{serialize_revisioned, deserialize_revisioned}` on `Preferences` (already derived).
- Derive `Clone` so the export can blank `available_themes`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M4.5

</details>

- [ ] Task 7 complete

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

- `cargo test --locked wiki_info_and_config`, `rss_feeds`, `search_encoding_and_comments`, `rewrite_emotes`, `flair_gallery_and_prefs_versioning`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Tasks 1–7

- **Supports:** M4.1, M4.2, M4.3, M4.4, M4.5

</details>

- [ ] Task 8 complete

## Acceptance Evidence

_Filled in when the tasks are complete._
