# Redlib Audit Remediation, Slice 2 — Task List

- **Status:** Ready
- **Author:** Argi
- **Created:** 2026-11-28
- **Language:** Rust
- **Source:** [plan.md](./plan.md), Slice 2 (S2): Security, Access Control, DoS Limits & Startup Binding Hardening

## Summary

Redlib is a public-facing proxy, so hostile input reaches it directly: crafted post bodies, redirect parameters, oversized requests, slow connections and scanner traffic. These tasks stop raw HTML in post bodies, off-site redirects, SFW-only bypasses and unbounded memory use, and make startup honour `--address`, strict boolean flags, a valid `redlib.toml` and a non-zero exit on server failure.

## Starting Context

Existing code the tasks build on (verified against `cc84f4c`; do not re-explore):

- `src/utils.rs:813-817` — `parse_post()` renders `selftext` containing a code fence with `pulldown_cmark::html::push_html`, which passes `Event::Html`/`Event::InlineHtml` through verbatim; the result is emitted with `{{ post.body|safe }}`. `static/style.css:1795` styles `.md .md-spoiler-text`, so spoiler markup needs a `.md` ancestor.
- `src/utils.rs:1304` — `redirect(path)` puts `path` in `Location` and, unescaped, in an HTML body. Callers: `src/main.rs`, `src/search.rs`, `src/subreddit.rs:106,217,358`, `src/settings.rs:85,131,304`, `src/utils.rs:992`.
- `src/settings.rs:119-131` — `set_cookies_method` builds the `redirect` target from the query string; `src/subreddit.rs:352` formats `"/{redirect_path}"`; `src/subreddit.rs:216` passes the `redir` query parameter straight to `redirect()`.
- `src/utils.rs:909` — `setting(req, name)` reads a cookie or the `REDLIB_DEFAULT_*` config value. `Preferences::new` (`src/utils.rs:697`) and `should_be_nsfw_gated` (`src/utils.rs:1379`) call it. `Post::fetch` (`src/utils.rs:356`) feeds listings, search and both RSS handlers; `subreddit()` and `user()` metadata failures become `unwrap_or_default()` at `src/subreddit.rs:112,116` and `src/user.rs:53`.
- `src/server.rs:443` — `#[cached] determine_compressor(String)` has an unbounded cache keyed on the `Accept-Encoding` header.
- `src/settings.rs:75-79` reads the whole `POST /settings` body with no cap; `src/settings.rs:276-298` (`encoded_restore`) reads the whole body before checking 1 MiB, then wraps synchronous work in `timeout(1s, async { ... })`. `src/utils.rs:752` — `deflate_decompress` uses `read_to_end` with no output cap.
- `src/main.rs:375,395` — the share-link and short-link handlers check only the length of the id before calling `canonical_path`, which sends up to six upstream HEAD requests.
- `src/config.rs:118-123` — `Config::load()` discards read and parse errors with `.ok()`; `CONFIG` is a `LazyLock` over `Config::load`.
- `src/main.rs:178-187` — `IPV4_ONLY`/`IPV6_ONLY` use `.is_ok()`; the flags replace `--address`. `src/main.rs:417` ignores a server error and exits 0. `src/server.rs:386-389` — `listen(&str)` panics on a bad address and `HyperServer::bind` panics when the port is taken; no `http1_header_read_timeout` is set.
- Slice 1 patterns: loopback sockets in `src/client.rs` tests; `text_response`/`fetch_text` tests in `src/main.rs`; `sealed_test` for env and cwd isolation in `src/config.rs`.

Conventions and gotchas:

- Tabs for indentation (`rustfmt.toml`). Windows builds need `cmake` and `nasm` on `PATH` (see [plan.md Prerequisites](./plan.md#prerequisites)).
- Tests never touch live Reddit: anything reaching `OAUTH_CLIENT` or `client::json` is out of bounds, so tests call pure helpers or handlers that return before the upstream call.
- Approved decisions: #3 escapes raw HTML events with no new crate; #8 fails startup non-zero. No new crates are needed.
- Supervisor decision: validate short-link ids as `[a-z0-9]` and share-link ids as `[A-Za-z0-9]`; add no denylist. All-lowercase scanner words such as `/login` and `/xmlrpc` are valid base36, so they still reach Reddit. The plan's acceptance row for those two paths is corrected to `/wp-admin`, `/.env` and `/Login`-style paths.

## Tasks

### Task 1: Redirects stay on this site and escape their fallback body

#### Problem

`utils::redirect()` trusts its argument. `/settings/update/?redirect=//evil.com`, a subscribe form with `redirect=/evil.com` (which becomes `//evil.com`) and the quarantine `redir=https://evil.com` all send visitors off-site, and the fallback HTML body interpolates the path unescaped.

#### Solution

- Add `sanitize_redirect_path(path) -> String` in `src/utils.rs`: return the path only when it starts with a single `/`, not `//` or `/\`, and has no control characters; otherwise return `/`.
- `redirect()` runs every target through it and HTML-escapes the fallback body, so every caller is covered without touching each one.
- Extract the subscribe/filter `redirect` handling into `subscription_redirect_path(query, sub)` so it can be tested without Reddit.

#### Notes

- **Location:** `src/utils.rs`, `src/subreddit.rs`, `src/settings.rs`

- **Verify:** `cargo test --locked --lib redirect_safety` passes.

- Boundary: pure functions plus `settings::update`/`subreddit::add_quarantine_exception` called directly. `subscriptions_filters` itself needs live Reddit; only its path helper is tested.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `src/utils.rs` tests and `src/settings.rs`/`src/subreddit.rs` tests, all named `redirect_safety_*`:

- `redirect_safety_rejects_offsite_targets`: `//evil.com`, `/\evil.com`, `https://evil.com`, `/ok\r\nSet-Cookie: x=1` give `Location: /`; `/r/rust?x=1&y=2` is kept.
- `redirect_safety_escapes_fallback_body`: a path with `"><script>` is absent from the body unescaped.
- `redirect_safety_settings_update`: `settings::update` with `?redirect=//evil.com` answers `Location: /`.
- `redirect_safety_quarantine_exception`: `add_quarantine_exception` with `redir=https://evil.com` answers `Location: /` and still sets the cookie.
- `redirect_safety_subscription_target`: `subscription_redirect_path("redirect=/evil.com", "rust")` is rejected by `redirect()`; `redirect=r/rust` gives `/r/rust`; no parameter gives `/r/rust`.

Red failure: the current `redirect()` returns the attacker's `Location` unchanged.

**Implementation guidance (green)**:

- `src/utils.rs:1304`: add the sanitiser and use `htmlescape::encode_attribute` for the body.
- `src/subreddit.rs:352`: replace the inline `path` computation with `subscription_redirect_path`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M2.2

</details>

- [x] Task 1 complete

---

### Task 2: Code-fence post bodies escape raw HTML and keep spoilers and autolinks

#### Problem

When a post's `selftext` contains a code fence, `parse_post()` re-renders it with `pulldown_cmark`, which copies raw HTML straight into the page. A body can inject `<meta http-equiv="refresh">` or `<style>`. The same branch drops Reddit's `>!spoiler!<` syntax and `r/`/`u/` links, because only Reddit's own `selftext_html` knows them.

#### Solution

- Add `render_markdown_body(selftext)` in `src/utils.rs`, used by the code-fence branch of `parse_post()`.
- It turns `Event::Html`/`Event::InlineHtml` into text events, so `push_html` escapes them.
- It rewrites `>!x!<` into `<span class="md-spoiler-text">x</span>` before parsing, outside fences, so a spoiler at the start of a line is not read as a block quote.
- It links `r/name`, `/r/name`, `u/name`, `/u/name` in text outside code and links.
- The result is wrapped in `<div class="md">` to match Reddit's markup and the spoiler CSS.

#### Notes

- **Location:** `src/utils.rs`

- **Verify:** `cargo test --locked --lib parse_post_code_fence` passes.

- Boundary: `parse_post` on a JSON fixture; no network. Link schemes such as `javascript:` in markdown links are not changed here; the CSP (`script-src 'self' blob:`) blocks them.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `src/utils.rs` tests, async, calling `parse_post(&fixture)`:

- `parse_post_code_fence_escapes_raw_html`: a body with a fence, `<meta http-equiv="refresh" content="0;url=//evil.com">`, and inline `<style>` has no `<meta` or `<style` in `post.body` and contains `&lt;meta`.
- `parse_post_code_fence_keeps_spoilers_and_autolinks`: `>!spoiler!<` at line start and mid-line (including bold inside it) become `md-spoiler-text` spans; `r/rust`, `/u/spez` and `u/ab_c` become `/r/rust` and `/u/...` links; text inside the fence and inline code stays literal.
- `parse_post_code_fence_keeps_ordinary_block_quotes`: a normal `> quote` is still a block quote.
- `parse_post_without_code_fence_uses_reddit_html` (control): bodies without a fence still use Reddit's `selftext_html` unchanged.

Red failure: `raw meta tag survived: <pre><code>code ... <meta http-equiv="refresh" ...>`, and a line-start spoiler rendered as `<blockquote><p>!whole line spoiler!&lt;</p>` with no `.md` wrapper.

**Implementation guidance (green)**:

- Pre-pass over lines toggling on ```` ``` ````/`~~~` fences; outside fences swap `>!...!<` for private-use sentinels (strip those characters from the input first). Restore them in code events.
- Merge adjacent text events before linking so `pulldown_cmark`'s text splitting cannot hide `r/name`.
- Track code-block and link/image depth; do not link inside either.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M2.1

</details>

- [x] Task 2 complete

---

### Task 3: SFW-only mode cannot be bypassed

#### Problem

With `REDLIB_SFW_ONLY=on`, a `show_nsfw=on` cookie still makes templates show NSFW posts in `/r/all`, `/r/popular`, multireddits and user listings. A failed subreddit or user metadata lookup defaults to `nsfw = false`, which skips the NSFW gate. RSS handlers never check.

#### Solution

- `setting()` returns `off` for `show_nsfw` on an SFW-only instance, so `Preferences::new`, `should_be_nsfw_gated` and every other reader agree.
- `Post::fetch` drops `over_18` posts on an SFW-only instance; listings, search and both RSS feeds share it. The listing parse moves into `Post::from_listing(&Value)` so it can be tested.
- A metadata lookup that fails on an SFW-only instance becomes an error page instead of a default (`gating_metadata`).
- Both RSS handlers refuse NSFW subreddits and users on an SFW-only instance.

#### Notes

- **Location:** `src/utils.rs`, `src/subreddit.rs`, `src/user.rs`

- **Verify:** `cargo test --locked --lib sfw_only_enforcement` passes.

- Boundary: env-isolated (`sealed_test`) tests on `Preferences::new`, `should_be_nsfw_gated`, `Post::from_listing` and `gating_metadata`. The handlers' wiring needs live Reddit and is review-only.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — all `sealed_test(env = [("REDLIB_SFW_ONLY", "on")])`, named `sfw_only_enforcement_*`:

- `..._ignores_show_nsfw_cookie`: a request with `Cookie: show_nsfw=on` gives `Preferences::new(..).show_nsfw == "off"` and `should_be_nsfw_gated` is true.
- `..._drops_nsfw_listing_posts`: `Post::from_listing` on a fixture with one `over_18` and one safe post returns only the safe one.
- `..._keeps_posts_when_instance_allows_nsfw`: positive control without the env var returns both posts.
- `..._honours_show_nsfw_cookie_when_instance_allows_it` (control, no env var): the cookie still works on a normal instance.
- `..._fails_closed_on_metadata_error`: `gating_metadata(Err("x"), true)` is `Err`; with `false` it is `Ok(default)`.

Red failure: `left: "on" right: "off"`, `left: ["nsfw1", "safe1"] right: ["safe1"]`, and the lookup error becoming `Ok(default)`.

**Implementation guidance (green)**:

- `src/utils.rs:909`: early return in `setting()` for `name == "show_nsfw"`.
- `src/subreddit.rs:112,116` and `src/user.rs:53`: use `gating_metadata`; reuse the existing subreddit error mapping for the failure page.
- `src/subreddit.rs:rss` and `src/user.rs:rss`: refuse when the metadata says NSFW and `sfw_only()`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M2.3

</details>

- [x] Task 3 complete

---

### Task 4: Request size and decompression limits

#### Problem

An attacker controls three things that grow memory: the `Accept-Encoding` header (cached forever, one entry per distinct value), request bodies for `POST /settings` and `/settings/encoded-restore` (read in full), and the output of `deflate_decompress` (no cap, so a few KB can expand to gigabytes). `timeout(1s, async { sync work })` cannot interrupt anything, because the async block never yields.

#### Solution

- Remove the cache on `determine_compressor`; parsing a header is cheap.
- Add `read_body_limited(body, max_bytes)` that errors as soon as the running total passes the cap, and use it in `settings::set` (64 KiB) and `settings::encoded_restore` (1 MiB).
- Replace `deflate_decompress` with `deflate_decompress_limited(input, max_bytes)` built on `Read::take`; `encoded_restore` allows 1 MiB.
- Delete both `timeout` wrappers; with the caps the synchronous work is small and bounded.

#### Notes

- **Location:** `src/server.rs`, `src/settings.rs`, `src/utils.rs`

- **Verify:** `cargo test --locked --lib dos_and_shortlink_limits` passes.

- Boundary: `settings::set`/`encoded_restore` driven with counting streaming bodies; pure functions for the rest.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — named `dos_and_shortlink_limits_*`:

- `..._accept_encoding_variants_are_parsed_independently` (`server.rs`): feeds 500 distinct headers, asserts each is parsed correctly. Red phase used a temporary assertion on the `cached` static's size (500 entries); the cache is gone in green, so the final test asserts parsing only.
- `..._settings_form_body_is_capped_while_streaming` (`settings.rs`): a 1 MiB stream of 16 KiB chunks makes `settings::set` return `Err` after reading well under the whole stream.
- `..._encoded_restore_body_is_capped_while_streaming` (`settings.rs`): same for a >1 MiB body.
- `..._encoded_restore_rejects_a_deflate_bomb` (`settings.rs`) and `..._deflate_output_is_capped` (`utils.rs`): 2 MiB of zeros compressed with `deflate_compress` makes `encoded_restore` and `deflate_decompress_limited` return `Err`; exactly the limit is accepted.
- `..._body_reader_stops_at_the_cap` (`utils.rs`): 1000 bytes pass a 1000-byte cap, not a 999-byte one.
- `..._settings_form_still_sets_cookies` (`settings.rs`, control): a normal form still sets its cookies.
- `..._encoded_restore_round_trips_preferences` (`settings.rs`): a valid base2048 payload still redirects to `/settings/restore/?...`.

Red failure: `the cache ... left: 500 right: 0`; `an oversized form must be refused` (set returned `Ok`); `pulled 128 chunks`; the bomb produced a `302` redirect instead of an error.

**Implementation guidance (green)**:

- `src/utils.rs`: `read_body_limited` loops on `HttpBody::data()`; `deflate_decompress_limited` reads through `take(max + 1)` and errors if more than `max` bytes come out. Update the three existing tests that call `deflate_decompress`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M2.4

</details>

- [x] Task 4 complete

---

### Task 5: Short and share links are validated before Reddit is called

#### Problem

Any 5–7 character path (`/admin`, `/xmlrpc`, `/Login`) and any 8–11 character `/r/:sub/s/:id` path reaches `canonical_path()`, which sends up to six HEAD requests to Reddit. Scanners trigger this constantly, which risks the instance's rate limit.

#### Solution

- Move the two inline route closures into `short_link` and `share_link` handlers in `src/main.rs`.
- `short_link` requires a 5–7 character id of `[a-z0-9]`; `share_link` requires 8–11 characters of `[A-Za-z0-9]`. Anything else gets the existing "Nothing here" page without an upstream call.
- Known limitation, by decision: all-lowercase words such as `login` are valid base36 and still reach Reddit. No denylist.

#### Notes

- **Location:** `src/main.rs`

- **Verify:** `cargo test --locked --bin redlib dos_and_shortlink_limits` passes.

- Boundary: handlers called with a hand-built request; invalid ids return the error page before any upstream call. A valid id would call live Reddit, so it is not exercised.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `src/main.rs` tests:

- `dos_and_shortlink_limits_non_base36_short_links_make_no_upstream_call`: `/Login`, `/wp-adm`, `/ab.cde`, `/abc-de` give a 404 page.
- `dos_and_shortlink_limits_malformed_share_ids_make_no_upstream_call`: ids with `-`, `.` or `%` give a 404 page.
- `dos_and_shortlink_limits_id_alphabets`: pure check of the accepted alphabets, including a mixed-case share id and an all-lowercase short id.

Red failure: with a length-only check the two handler tests hang for over 60 s inside `canonical_path()` (it initialises the live OAuth client), and `id_alphabets` fails. Restoring the alphabet check makes them return the 404 page at once.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M2.4

</details>

- [x] Task 5 complete

---

### Task 6: An unreadable or invalid `redlib.toml` stops startup

#### Problem

`Config::load()` drops read and parse errors, so a typo in `redlib.toml` silently turns off `REDLIB_SFW_ONLY` or `REDLIB_ROBOTS_DISABLE_INDEXING`.

#### Solution

- Add `Config::try_load() -> Result<Self, String>`. A missing `redlib.toml`/`libreddit.toml` is fine; an existing file that cannot be read or parsed is an error naming the file.
- `Config::load()` (used by the `CONFIG` static) logs that error and exits with status 1. `main` forces `CONFIG` first thing so the failure happens at startup.

#### Notes

- **Location:** `src/config.rs`, `src/main.rs`

- **Verify:** `cargo test --locked --lib startup_config_and_bind` passes.

- Boundary: `Config::try_load` on temp-dir files via `sealed_test`; the process exit itself is not spawned.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — `src/config.rs`, `sealed_test`, named `startup_config_and_bind_*`:

- `..._rejects_malformed_toml`: `redlib.toml` containing `REDLIB_SFW_ONLY = ` gives `Err` mentioning `redlib.toml`.
- `..._rejects_unreadable_config`: `redlib.toml` that is a directory gives `Err`.
- `..._accepts_missing_files`: no files gives `Ok` with all `None`.
- `..._rejects_malformed_legacy_file`: the same for `libreddit.toml`.
- `..._reads_valid_file`: a valid file still yields its value.

Red failure: `unwrap_err()` on an `Ok` value (the stub `try_load` wrapped the old `load`, which ignores errors).

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M2.5

</details>

- [x] Task 6 complete

---

### Task 7: Flags and `--address` agree, and server failures exit non-zero

#### Problem

`IPV4_ONLY=false` still turns IPv4-only mode on, IPv4-only mode replaces `--address 127.0.0.1` with `0.0.0.0` (exposing a loopback-only deployment), and setting both flags prefers IPv4 silently. A bad address panics, an occupied port panics, and a server error prints a message and exits 0, so `restart: on-failure` never fires.

#### Solution

- `cli()` builds the clap `Command`; `-4`/`-6` become `SetTrue` flags with `IPV4_ONLY`/`IPV6_ONLY` env vars, which clap parses as strict booleans. `--address` loses its clap default so an explicit value can be told from none.
- `resolve_listener(address: Option<&str>, port, ipv4_only, ipv6_only) -> Result<SocketAddr, String>` decides the bind address: conflicting flags or an address of the wrong family is an error; an explicit address is never replaced.
- `Server::listen(SocketAddr)` uses `try_bind` and returns an error future instead of panicking. `main` returns `ExitCode::FAILURE` on any startup or server error.

#### Notes

- **Location:** `src/main.rs`, `src/server.rs`

- **Verify:** `cargo test --locked --bin redlib startup_config_and_bind` and `cargo test --locked --lib startup_config_and_bind` pass.

- Boundary: `cli()` parsing under `sealed_test` env; `resolve_listener` pure; an occupied port via a real loopback listener. The exit-code mapping in `main` is a three-line `match`, reviewed rather than spawned.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)** — named `startup_config_and_bind_*`:

- `..._false_env_does_not_enable_ipv4_only` (`sealed_test`, `IPV4_ONLY=false`): the listener stays `[::]:8080`; `..._true_env_enables_ipv4_only` (control) gives `0.0.0.0:8080`.
- `..._invalid_env_value_is_rejected`: `IPV6_ONLY=maybe` makes argument parsing fail.
- `..._explicit_address_is_kept_with_family_flag` (`IPV4_ONLY=1`): `-a 127.0.0.1` stays `127.0.0.1:8080`; `-4 -a 192.168.1.5 -p 9000` keeps its address and port.
- `..._family_flags_pick_the_wildcard_address`: no flag and `-6` give `[::]`, `-4` gives `0.0.0.0`, and `[::1]`/`::1` are accepted.
- `..._conflicting_flags_are_errors` and `..._conflicting_env_flags_are_errors`: `-4 -6`, both env vars, `-4 -a ::1` and `-6 -a 127.0.0.1` are `Err`.
- `..._invalid_address_or_port_is_an_error_not_a_panic`: the error names the bad value.
- `..._occupied_port_is_an_error` (`server.rs`): `listen` on a bound loopback port resolves to `Err`.

Red failure: `left: 0.0.0.0:8080 right: [::]:8080`, `left: 0.0.0.0:8080 right: 127.0.0.1:8080`, no error for `-4 -6`; `listen` panicked with `error binding to 127.0.0.1:59824 ... (os error 10048)`.

Green notes: clap's default parser for a `SetTrue` flag accepts only `true`/`false` from the environment, so the flags use `BoolishValueParser` (`1`, `0`, `on`, `off`, ...).

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 6 (same file, `main.rs` startup order)

- **Supports:** M2.5

</details>

- [x] Task 7 complete

---

### Task 8: Connections that send headers too slowly are closed

#### Problem

hyper 0.14 sets no limit on how long a client may take to send request headers, so a few hundred sockets that dribble bytes can hold every connection slot on a public instance (slowloris).

#### Solution

- Add `Server.header_read_timeout` (default 30 s, public like `default_headers`) and pass it to `http1_header_read_timeout` when serving. hyper needs the tokio timer; the `tokio` `full` feature already provides it.

#### Notes

- **Location:** `src/server.rs`

- **Verify:** `cargo test --locked --lib http1_header_timeout` passes.

- Boundary: a loopback server and a raw TCP client that sends part of a request and stops.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Test (red)** — `src/server.rs`, `http1_header_timeout_closes_slow_clients`: set `header_read_timeout` to 200 ms, `listen` on a free loopback port, connect, send `GET / HTTP/1.1\r\nHost: x\r\n` without the final blank line, then read to EOF inside a 5 s guard timeout. Assert the read ended (EOF or a response) before the guard fired.

Red failure: `the server kept a connection open that never finished its headers` after the 5 s guard. (The field was added as an unused stub first so the test compiled.)

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 7 (`listen` signature)

- **Supports:** M2.6

</details>

- [x] Task 8 complete

---

### Task 9: Integration / Boundary Verification

#### Problem

Slice 2 is accepted only when every milestone (M2.1–M2.6) has a passing regression test and the repository's format and lint checks stay clean. No living `.feature` files exist in this repository, so the executable assertions are the Rust tests named below.

#### Solution

- Run format, clippy and the full offline suite.
- Confirm each milestone's tests ran by name.
- Review-only items: handler wiring that needs live Reddit (SFW fail-closed in `community`/`profile`, RSS refusals, `subscriptions_filters`).

#### Notes

- **Location:** whole crate

- **Verify:** all three commands exit 0 and the output lists the tests below as `ok`.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Commands**:

- `cargo fmt --check`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo test --locked`

**Milestone evidence**: see the Acceptance Evidence section below.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Tasks 1–8

- **Supports:** M2.1, M2.2, M2.3, M2.4, M2.5, M2.6

</details>

- [x] Task 9 complete

## Acceptance Evidence

Slice 2 implemented on 2026-11-28 against baseline `cc84f4c` (Slice 1).

- **Checks:** `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D warnings` clean; `cargo test --locked` passes 96 library tests and 19 binary tests offline, with the 20 live-Reddit tests ignored.
- **Milestone filters (plan commands):** `parse_post_code_fence` 3 passed; `redirect_safety` 5; `sfw_only_enforcement` 7; `dos_and_shortlink_limits` 8 (lib) + 3 (bin); `startup_config_and_bind` 11 (lib) + 13 (bin); each `sealed_test` is counted twice because its forked child reports too; `http1_header_timeout` 1.
- **Binary smoke (no network needed, they fail before any upstream call):** a malformed `redlib.toml` prints the parse error and exits 1; `-4 -6`, `-a nonsense` exit 1 with a message; `IPV4_ONLY=maybe` exits 2 with clap's error.

### Red-green

Every regression test was seen failing before its fix, except where noted.

| Milestone | Test(s) | Observed red failure |
| --- | --- | --- |
| M2.2 | `redirect_safety_*` (5) | `"//evil.com" must not be followed left: "//evil.com" right: "/"`; fallback body contained the raw `<script>`; quarantine `redir=https://evil.com` gave `Location: https://evil.com` |
| M2.1 | `parse_post_code_fence_escapes_raw_html`, `..._keeps_spoilers_and_autolinks` | `raw meta tag survived`; line-start spoiler became a `<blockquote>` and no `.md` wrapper |
| M2.3 | `sfw_only_enforcement_*` (3 of 7; the others are positive controls that passed before) | `show_nsfw` `"on"` vs `"off"`; both posts returned; metadata error became `Ok(default)` |
| M2.4 | `..._accept_encoding_variants_...` | 500 cache entries (temporary assertion on the `cached` static; the final test asserts parsing only, which cannot fail on the old code) |
| M2.4 | `..._settings_form_body_is_capped_while_streaming`, `..._encoded_restore_body_is_capped_while_streaming`, `..._encoded_restore_rejects_a_deflate_bomb`, `..._deflate_output_is_capped`, `..._body_reader_stops_at_the_cap` | `Ok` returned after pulling all 128 chunks; bomb redirected with `302`; `unwrap_err()` on an `Ok` value |
| M2.4 | `dos_and_shortlink_limits_*` in `main.rs` | handlers hang >60 s reaching the live OAuth client; alphabet test failed (seen by temporarily restoring the length-only check after the fix, because the fix and test were first written together) |
| M2.5 | `startup_config_and_bind_*` in `config.rs`, `main.rs`, `server.rs` | see Task 6 and Task 7 |
| M2.6 | `http1_header_timeout_closes_slow_clients` | connection still open after the 5 s guard |

### Corrections to the plan

- The plan's M2.4 row asked for `/login` and `/xmlrpc` to make zero Reddit calls. Both are valid base-36 strings, and the supervisor chose base-36 validation without a denylist, so the row is corrected to `/Login`, `/wp-adm`, `/ab.cde`, `/abc-de`. All-lowercase scanner words still reach Reddit; this is a known limitation.
- #7 is enforced in `utils::setting()` rather than `Preferences::new()`. `Preferences::new`, `should_be_nsfw_gated` and the `all_posts_hidden_nsfw` checks all read through it, so one early return covers them.
- The `timeout(1s, async { sync })` wrappers are removed rather than moved to `spawn_blocking`: with the 1 MiB caps the remaining synchronous work is tiny.
- `Server::listen` now takes a `SocketAddr` and returns the bind error through its future; `deflate_decompress` is replaced by `deflate_decompress_limited` (its three existing tests were updated to call it).
- No finding was invalid or already fixed.

### Not verified

- The wiring in `subreddit::community`, `user::profile` and both RSS handlers (fail-closed metadata, NSFW RSS refusal) needs live Reddit; the pure pieces they call are tested. `subscriptions_filters` itself needs live Reddit; its path helper is tested.
- The `main()` mapping from a startup error to `ExitCode::FAILURE` is not unit tested. `tests/startup_config.rs` covers the config failure path (exit 1) through the binary.
- Valid short and share ids reaching `canonical_path()` were not exercised (live Reddit).
- Markdown links with a `javascript:` scheme are not rewritten; the CSP (`script-src 'self' blob:`) blocks them.
- Slowloris behaviour was tested on Windows with Tokio's timer; hyper's header timeout is platform independent.

### Review

A reviewer read the slice. No Critical findings. Verified findings and their dispositions:

| Finding | Disposition | Regression test |
| --- | --- | --- |
| An empty `IPV4_ONLY=` or `IPV6_ONLY=` crashed startup (exit 2, variable not named); before the slice any set value meant true | Fixed: `parse_flag` keeps the strict booleans and treats an empty value as false | `startup_config_and_bind_empty_env_value_counts_as_unset` (red: `invalid value for one of the arguments`) |
| The fail-closed config path (`Config::load`, forcing `CONFIG`, exit code) had no end-to-end test | Fixed: the test runs the built binary in a temp dir with a malformed `redlib.toml` | `startup_config_malformed_file_stops_startup_with_exit_1_and_names_the_file` (red with `load` swallowing the error: the binary kept running until the 20 s guard) |
| `--help`/`--version` forced the config first, so a bad `redlib.toml` blocked them | Fixed: `main` parses arguments before forcing `CONFIG` | `startup_config_version_works_with_a_malformed_file` (red: exit 1 with the parse error). `--help` is not enabled in this clap build, so only `--version` is covered |
| The redirect tests missed a tab case and accepted any escaping of the fallback body | Fixed: added `"/	/evil.com"`; the body is asserted exactly | `redirect_safety_rejects_offsite_targets`, `redirect_safety_escapes_fallback_body` (red when the check is narrowed to CR/LF, and when `encode_attribute` is swapped for `encode_minimal`) |
| Subreddit search listed NSFW communities on an SFW-only instance | Fixed: `subreddit_results` drops `over18` entries when `sfw_only()` | `sfw_only_search_results_leave_out_nsfw_communities` (red: `["family", "adult"]` vs `["family"]`) |

Accepted, not fixed:

- The Accept-Encoding cache test only checks parsing, so re-adding `#[cached]` to `determine_compressor` would pass it.
- `DEFAULT_HEADER_READ_TIMEOUT` is not exercised: the timeout test sets its own value.
- The SFW-only wiring in `subreddit::community`, `user::profile` and both RSS handlers stays review-only (it needs live Reddit or a `json()` seam).
- Not verified: whether the wiki, `/about` and user-comment listings need NSFW gating on SFW-only instances; the Reddit payload for `t1` items was not checked.
