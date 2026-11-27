# Redlib Audit Remediation, Slice 3 — Task List

- **Status:** Complete
- **Author:** Argi
- **Created:** 2026-11-28
- **Language:** Rust
- **Source:** [plan.md](./plan.md), Slice 3 (S3): Server Performance, Panic Isolation, Observability & Healthcheck

## Summary

Redlib answers every busy page with work that blocks Tokio workers (Brotli at quality 11, one `Preferences` parse per comment, an uncached 412 KB script on every listing) and fails quietly when something breaks (panics reset sockets, render errors serve blank `200` pages, nothing is logged, `/settings` stays "healthy" with dead OAuth). These tasks move compression off the executor, parse cookies once per thread, cache static scripts, return `500` for panics and render failures, add privacy-safe request logs, a cheap `/healthz`, a `--healthcheck` flag for containers and a bounded shutdown.

## Starting Context

Existing code the tasks build on (verified against `04003d1`; do not re-explore):

- `src/server.rs` — `compress_response` consumes the body and calls the `#[cached(size = 100, time = 600)]` `compress_body`, which keys on the whole `Vec<u8>` body and encodes Brotli with `BrotliEncoderParams::default()` (quality 11) on the async worker. Callers discard its `Err` with `let _ =`, so a failed compression leaves an empty body. The service closure in `Server::listen` routes, awaits the handler, then compresses. `Server.header_read_timeout` and `listen(SocketAddr)` came from Slice 2; the graceful-shutdown future waits for every connection with no deadline.
- `src/server.rs` — `ResponseExt::cookies()` reads the request `Cookie` header from a *response*, and nothing calls it.
- `src/utils.rs` — `template()`, `error()`, `info()`, `nsfw_landing()` and `subreddit::quarantine()` call `render().unwrap_or_default()`, so a render error is a blank page with no log. `Comment.prefs: Preferences` is set per comment in `post::build_comment`; `templates/comment.html` reads only `prefs.hide_score` and `prefs.hide_awards`.
- `src/post.rs` — `parse_comments`/`query_comments` copy the children array (`as_array().map_or(Vec::new(), ToOwned::to_owned)`).
- `src/main.rs` — `run()` registers every route inline; JS routes call `resource(.., false)` (no `Cache-Control`); templates load `/hls.min.js` and `/playHLSVideo.js` whenever `prefs.use_hls == "on"` (`templates/subreddit.html:67`, `search.html:109`, `user.html:84`) and per HLS post in `templates/utils.html:125-132`.
- `src/client.rs` — `json()` is `#[cached(size = 100, time = 30, result = true)]` with no write coalescing. `OAUTH_RATELIMIT_REMAINING` and `OAUTH_CLIENT` are statics; `Oauth.expires_in` is private to `src/oauth.rs`. `force_refresh_token` swaps `OAUTH_CLIENT` after a refresh.
- `src/subreddit.rs:289` — `subscriptions_filters` fetches `/r/{sub}/hot.json` only to map names to display names, then falls back to `about.json` per name anyway.
- `src/oauth.rs` tests hold a private `CapturingLogger`; `log::set_logger` works once per process, so a second logger in the lib tests would be silently ignored.
- Slice 1-2 patterns: loopback sockets with a released port (`src/server.rs` tests), `tests/startup_config.rs` runs the built binary.

Conventions and gotchas:

- Tabs for indentation (`rustfmt.toml`). Windows builds need `cmake` and `nasm` on `PATH` (see [plan.md Prerequisites](./plan.md#prerequisites)).
- Tests never touch live Reddit. Nothing here can reach `OAUTH_CLIENT` or `client::json` in a test.
- Telemetry rules (service-telemetry skill): never log IPs, cookies, query strings, tokens or raw paths. Routine per-request success is `debug`; failures are logged once at their owning boundary as `warn` or `error`.
- No new crates; `hyper::Client`, `tokio` and `futures-lite` are already dependencies.
- Plan signatures are proposals: `compress_body_blocking` takes `&[u8]` (so the original bytes survive for the error path) and the cache is removed rather than hash-keyed.

## Tasks

### Task 1: Compression runs off the executor, keeps the body on failure and sets `Vary`

#### Problem

For every HTML or JSON response of 1452 bytes or more, `compress_response` runs Brotli at quality 11 on the Tokio worker that is serving requests, stalling it for tens to hundreds of milliseconds. It also caches whole uncompressed bodies as `Vec<u8>` keys (hashing and cloning each one), and when compression fails after the body was read, the response goes out with an empty body. Compressed responses carry no `Vary: Accept-Encoding`, so a shared cache may hand a Brotli body to a client that did not ask for it.

#### Solution

- `compress_body_blocking(compressor, &[u8])` replaces the cached `compress_body`: no cache, Brotli quality 4 with `lgwin` 20.
- A new `compress_body_in_place(res, compressor)` reads the body as `Bytes`, compresses a clone with `spawn_blocking`, and on error puts the original bytes back.
- `compress_response` appends `Vary: Accept-Encoding` to every response that is eligible for compression (text or JSON, 1452 bytes or more), compressed or not.

#### Notes

- **Location:** `src/server.rs`

- **Verify:** `cargo test --locked --lib compress_response` passes when the new tests and the updated round-trip test pass.

- Boundary: `compress_response` called directly on built responses inside a Tokio test runtime. No network.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — extract the current consume-and-compress step into `compress_body_in_place(res, compressor)` with today's behaviour (body dropped on error), so the tests compile.

**Tests (red)** — `src/server.rs` tests, all `compress_response_*`:

- `compress_response_restores_the_body_when_compression_fails`: `compress_body_in_place(&mut res, CompressionType::Passthrough)` returns `Err`, the body bytes equal the original and no `Content-Encoding` is set.
- `compress_response_does_not_block_the_async_executor`: on a current-thread runtime, `join!` the compression with a task that sets a flag; the compression future reports whether the flag was already set when it finished. Inline compression finishes in its first poll, so the flag is unset.
- `compress_response_sets_vary_for_compressible_responses`: `Vary: Accept-Encoding` is present on a compressed response and on an eligible response sent without `Accept-Encoding`; absent on a small response and on `image/png`.
- `test_compress_response` (the existing round trip) becomes `#[tokio::test]` because compression now needs a runtime.

Red failures: `body was dropped`; `compression ran inline on the executor`; `Vary header missing`.

**Implementation guidance (green)**:

- `body::to_bytes` gives `Bytes`; clone it into the `spawn_blocking` closure and restore the original on `Err` or a `JoinError`.
- Remove the `cached` import from `server.rs` once `compress_body` is gone.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M3.1, M3.6

</details>

- [x] Task 1 complete

---

### Task 2: Comment rendering parses cookies once per request

#### Problem

`post::build_comment` calls `Preferences::new(req)` for every comment. Each call rebuilds the theme list and parses the `Cookie` header about 25 times, so a 1,000-comment thread does that work 1,000 times for two flags (`hide_score`, `hide_awards`). `parse_comments` and `query_comments` also copy the whole children array before iterating it.

#### Solution

- `parse_comments`, `query_comments` and `build_comment` take `&Preferences` instead of the request; `item()` builds one `Preferences` and reuses it for the page template.
- `Comment` stores `hide_score` and `hide_awards` as booleans instead of a whole `Preferences`; `comment.html` reads them.
- Children are iterated by reference.

#### Notes

- **Location:** `src/post.rs`, `src/utils.rs`, `templates/comment.html`

- **Verify:** `cargo test --locked --lib comment_prefs_reuse` passes.

- Boundary: `parse_comments` on a 200-comment JSON fixture, rendered through the `Comment` template. No request object exists in the test, which is the point.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — change the three signatures to take `&Preferences`, add `Comment.hide_score`/`hide_awards` (set `false`), and update the template. The stub ignores the preferences.

**Tests (red)** — `src/post.rs`, `comment_prefs_reuse_*`:

- `comment_prefs_reuse_hides_score_and_awards_for_every_comment`: 200 sibling comments with awards, `Preferences { hide_score: "on", hide_awards: "on", .. }`; every parsed comment renders `&#x2022;` and no `class="award"`.
- `comment_prefs_reuse_shows_them_by_default`: the same fixture with default preferences renders the score and the award (positive control).

Red failure: the hidden-score comments rendered their score text.

**Implementation guidance (green)**:

- `build_comment` sets `hide_score: prefs.hide_score == "on"` and likewise `hide_awards`.
- `json["data"]["children"].as_array().into_iter().flatten()` replaces the cloned `Vec`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M3.2, M3.6

</details>

- [x] Task 2 complete

---

### Task 3: Handler panics and render failures return `500` and are logged

#### Problem

A panic inside a handler drops the TCP connection, so the visitor sees a reset instead of an error page, and nothing says which route failed. Separately, `render().unwrap_or_default()` turns an Askama failure into a blank `200` page.

#### Solution

- The service wraps route dispatch (the call and the awaited future) in `catch_unwind`, logs `error!` with the method and route template, and answers `500` with a plain-text body.
- `utils::html_page(rendered, status)` turns a render result into a response: the page on success, a logged `500` plain-text fallback on failure. `template()`, `error()`, `info()`, `nsfw_landing()` and `quarantine()` use it.
- The router stores each route's template next to its handler so logs can name the route without the visitor's path.

#### Notes

- **Location:** `src/server.rs`, `src/utils.rs`, `src/subreddit.rs`

- **Verify:** `cargo test --locked --lib panic_and_render_errors` passes.

- Boundary: a loopback `Server` with routes that panic (synchronously and inside the future), and `html_page` with an `askama::Error`. `instance_info` still unwraps its renders; a panic there is now caught and answered with `500`.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — `html_page(rendered, status)` that returns `rendered.unwrap_or_default()` with `status`, so the test compiles.

**Tests (red)**:

- `src/utils.rs` `panic_and_render_errors_failed_render_is_a_500`: `html_page(Err(askama::Error::Fmt), 200)` has status `500`, content type `text/plain` and no HTML. `panic_and_render_errors_successful_render_keeps_status`: `html_page(Ok("<p>x</p>"), 403)` keeps `403` and `text/html`.
- `src/server.rs` `panic_and_render_errors_panicking_route_returns_500`: a loopback server with `/sync-panic` (panics when called), `/async-panic` (panics when polled) and `/ok`; both panics answer `500` and `/ok` still answers `200`.

Red failures: status `200` vs `500`; the client request errors with a closed connection.

**Implementation guidance (green)**:

- `AssertUnwindSafe(async move { func(req).await }).catch_unwind()` covers both panic sites.
- The panic message is already printed by the default panic hook; the `error!` line adds only the method and route template.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 1 (shared `compress_response` call in the service)

- **Supports:** M3.4

</details>

- [x] Task 3 complete

---

### Task 4: Static scripts are cached and `hls.min.js` loads only with HLS video

#### Problem

Every JS route is served without `Cache-Control`, including the 412 KB `hls.min.js`, and subreddit, search and user pages load it synchronously whenever the `use_hls` preference is on, even when no post has an HLS stream.

#### Solution

- `main.rs` builds the application in `build_app(hsts)`, so tests can serve the real route table on a loopback port.
- JS routes use the existing cache header (`public, max-age=1209600, s-maxage=86400`), and templates reference them as `/name.js?v={{ env!("CARGO_PKG_VERSION") }}` like `style.css`.
- `utils::any_hls_video(posts)` is true when a post has an HLS `alt_url`; the three listing templates add the HLS scripts only when `use_hls` is on and it is true, with `defer`.

#### Notes

- **Location:** `src/main.rs`, `src/utils.rs`, `templates/*.html`

- **Verify:** `cargo test --locked --bin redlib static_js_caching` and `cargo test --locked --lib static_js_caching` pass.

- Boundary: a loopback request to the real route table, and the listing templates rendered from fixtures.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**:

- `src/main.rs` `static_js_caching_scripts_carry_cache_control`: `GET /hls.min.js`, `/playHLSVideo.js`, `/highlighted.js`, `/check_update.js`, `/copy.js` all answer `Cache-Control` containing `max-age`.
- `src/subreddit.rs`, `src/search.rs`, `src/user.rs` `static_js_caching_*_loads_hls_only_for_hls_posts`: each template renders with `use_hls: "on"` and a post with `alt_url` (contains `/hls.min.js?v=`) and without (does not contain `hls.min.js`).

Red failures: `missing Cache-Control for /hls.min.js`; the page without HLS posts still contained `/hls.min.js`.

**Implementation guidance (green)**:

- Hoist `run`'s route registration and default headers into `build_app`.
- In `utils.html` the per-post HLS script tags gain the version query and `defer`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 3 (route table type)

- **Supports:** M3.3

</details>

- [x] Task 4 complete

---

### Task 5: Privacy-safe request logs

#### Problem

Redlib logs nothing about requests, so an operator cannot see error rates, which routes fail or how long they take.

#### Solution

- After each response the service logs at `debug`: method, route template, status and duration in milliseconds. Never the path, query, headers, cookies or client address.
- A handler `Err` logs at `warn` with the same fields and no message (the message can echo a search term); the message goes to `debug`.
- The lib tests share one capturing logger (`src/test_logging.rs`) because the process allows only one logger; the `oauth.rs` tests switch to it.

#### Notes

- **Location:** `src/server.rs`, `src/lib.rs`, `src/test_logging.rs`, `src/oauth.rs`

- **Verify:** `cargo test --locked --lib request_logs` passes.

- Boundary: a loopback server and the shared test logger.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Test (red)** — `src/server.rs`, `request_logs_record_the_route_template_without_personal_data`: request `GET /r/privatesub/comments/abc123?q=secretterm` with `Cookie: theme=secretcookie` against `/r/:sub/comments/:id`. A captured `debug` line contains `GET`, `/r/:sub/comments/:id`, `200` and `ms`; no captured line contains `privatesub`, `secretterm`, `secretcookie` or `127.0.0.1`. A second case, an unmatched path, logs the route as `unmatched`.

Red failure: `no request log line was emitted`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 3

- **Supports:** M3.5

</details>

- [x] Task 5 complete

---

### Task 6: `/healthz` reports OAuth state without calling Reddit

#### Problem

The container healthcheck requests `/settings`, which succeeds even when the OAuth token has expired and every Reddit page fails.

#### Solution

- `src/health.rs` adds `HealthReport` and `healthz()`. It reads two in-memory values and never calls Reddit: the token expiry time (a new `OAUTH_TOKEN_EXPIRES_AT` static, written when a client is installed) and `OAUTH_RATELIMIT_REMAINING`.
- Body: `{"status":"ok"|"degraded","oauth_ready":bool,"ratelimit_remaining":u16}`. `200` when the token is unexpired, `503` otherwise. The rate limit count is informational.
- `main.rs` registers `/healthz`.

#### Notes

- **Location:** `src/health.rs`, `src/client.rs`, `src/oauth.rs`, `src/main.rs`

- **Verify:** `cargo test --locked --lib healthz_and_shutdown_report` and `cargo test --locked --bin redlib healthz_and_shutdown` pass.

- Boundary: `HealthReport::assess` as a pure function, and `GET /healthz` through the real route table with the expiry static set directly. The wiring that records the expiry when a token is installed needs live Reddit and is review-only.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**:

- `src/health.rs` `healthz_and_shutdown_report_ok_with_a_live_token` and `..._report_degraded_when_the_token_expired_or_missing`: an expiry in the future gives `ok`/`true`; the past and `0` give `degraded`/`false`.
- `src/main.rs` `healthz_and_shutdown_route_answers_200_then_503`: with the expiry static in the future `GET /healthz` is `200` and the JSON parses; after setting it to `1`, `503`. The route must win over `/:id`.

Red failure: `/healthz` answered `404` ("Nothing here").

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 4 (`build_app`)

- **Supports:** M3.5

</details>

- [x] Task 6 complete

---

### Task 7: `redlib --healthcheck` probes the local `/healthz`

#### Problem

The Docker images have no `curl` or `wget`, so their healthchecks need the binary itself to ask "is the server up and healthy?".

#### Solution

- `--healthcheck` requests `http://<local address>:<port>/healthz` using the same `--port`/`PORT`/`--address` resolution as the server, prints `ok` on stdout and exits `0` for a `200`; otherwise it prints the reason on stderr and exits `1`. Clap keeps exit `2` for usage errors.
- It runs before config loading and OAuth, so it needs nothing beyond loopback. A wildcard listen address is probed on `127.0.0.1` (and `::1` when the server listens on `[::]`), with a 5 s limit.

#### Notes

- **Location:** `src/health.rs`, `src/main.rs`, `tests/healthcheck.rs`

- **Verify:** `cargo test --locked --test healthcheck` and `cargo test --locked --lib healthz_and_shutdown_probe` pass.

- Boundary: the built binary against a stub HTTP server on a loopback port; address selection as a pure function.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**:

- `tests/healthcheck.rs` `healthz_and_shutdown_healthcheck_exits_0_for_200_and_1_for_503_or_no_server`: a stub answering `200` gives exit 0 and `ok`; `503` gives exit 1 and the status on stderr; a released port gives exit 1.
- `src/health.rs` `healthz_and_shutdown_probe_addresses_*`: `0.0.0.0` gives `127.0.0.1`; `::` gives `127.0.0.1` then `::1`; an explicit address is kept.

Red failure: the binary rejected `--healthcheck` with exit 2 (`unexpected argument`).

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 6

- **Supports:** M3.5

</details>

- [x] Task 7 complete

---

### Task 8: Graceful shutdown ends at a deadline

#### Problem

After SIGINT or SIGTERM the server stops accepting connections but waits for every open one. A stalled client or a long proxied stream keeps the process alive until Docker sends `SIGKILL`.

#### Solution

- `Server.shutdown_deadline` (default 5 s) bounds the wait. `listen_until(addr, shutdown)` serves until `shutdown` resolves, then waits at most the deadline for open connections before returning `Ok(())`; `listen(addr)` passes the Ctrl+C/SIGTERM future.

#### Notes

- **Location:** `src/server.rs`

- **Verify:** `cargo test --locked --lib healthz_and_shutdown_stalled` passes.

- Boundary: a loopback server, a client that sends half a request and stalls, and a oneshot shutdown signal. The real signal handlers are not exercised.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — add the field and `listen_until` with plain graceful shutdown.

**Test (red)** — `src/server.rs`, `healthz_and_shutdown_stalled_connection_does_not_block_shutdown`: `header_read_timeout` 60 s, `shutdown_deadline` 200 ms, a stalled client, then the shutdown signal. The `listen_until` future must finish within 5 s.

Red failure: `shutdown waited for a stalled connection` after the 5 s guard.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 3

- **Supports:** M3.5

</details>

- [x] Task 8 complete

---

### Task 9: Single-flight `json`, cheap display-name lookup, working `ResponseExt::cookies`

#### Problem

Concurrent identical requests each spend one of Reddit's rate-limit slots; subscribing to a subreddit downloads its whole hot listing just to learn name capitalisation; and `ResponseExt::cookies()` reads the request `Cookie` header from a response, so it never returns what the response sets.

#### Solution

- `client::json` uses `sync_writes = "by_key"`, so concurrent calls for one `(path, quarantine)` key share one upstream request.
- `display_names(sub, fetch)` resolves each name through `about.json` and no longer requests `hot.json`.
- `ResponseExt::cookies()` parses `Set-Cookie` headers.

#### Notes

- **Location:** `src/client.rs`, `src/subreddit.rs`, `src/server.rs`

- **Verify:** `cargo test --locked --lib vary_singleflight_and_cookies` passes.

- Boundary: `json` itself needs live Reddit, so the coalescing test uses a local function with the same `#[cached(...)]` attributes (it proves the attribute combination, not the call site). The lookup takes the fetcher as a closure.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**:

- `src/client.rs` `vary_singleflight_and_cookies_concurrent_identical_calls_fetch_once`: 10 concurrent calls to a local `#[cached(size = 100, time = 30, result = true, sync_writes = "by_key")]` function run its body once. Red with the attribute removed: the body ran 10 times.
- `src/subreddit.rs` `vary_singleflight_and_cookies_display_names_use_about_json`: the fake fetcher records requested paths and returns a `display_name`; the only path is `/r/rust/about.json?raw_json=1` and the result carries the display name. `u_` names are not fetched.
- `src/server.rs` `vary_singleflight_and_cookies_response_cookies_reads_set_cookie`: after `insert_cookie`, `cookies()` returns the cookie with its name and value; a response with only a `Cookie` header returns none.

Red failures: body ran 10 times; recorded path `/r/rust/hot.json?raw_json=1`; `cookies()` returned an empty list.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 1

- **Supports:** M3.6

</details>

- [x] Task 9 complete

---

### Task 10: Integration / Boundary Verification

#### Problem

Slice 3 is accepted only when every milestone (M3.1–M3.6) has a passing regression test and the format and lint checks stay clean. No living `.feature` files exist in this repository, so the executable assertions are the Rust tests named above.

#### Solution

- Run format, clippy and the full offline suite, and confirm each milestone's tests ran by name.
- Review-only: the OAuth expiry wiring (needs live Reddit) and the real SIGINT/SIGTERM handlers.

#### Notes

- **Location:** whole crate

- **Verify:** all three commands exit 0 and list the tests above as `ok`.

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

- **Blocked by:** Tasks 1–9

- **Supports:** M3.1, M3.2, M3.3, M3.4, M3.5, M3.6

</details>

- [x] Task 10 complete

## Acceptance Evidence

Slice 3 implemented on 2026-11-28 against baseline `04003d1` (Slice 2).

- **Checks:** `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D warnings` clean; `cargo test --locked` passes 117 library tests, 23 binary tests, 3 `healthcheck` and 2 `startup_config` integration tests offline, with the 20 live-Reddit tests ignored.
- **Milestone filters:** `compress_response` 4; `comment_prefs_reuse` 2; `static_js_caching` 3 (lib) + 1 (bin); `panic_and_render_errors` 3; `request_logs` 1; `healthz_and_shutdown` 2 + 1 probe (lib), 1 route test (bin), 3 (`--test healthcheck`), 1 stalled-shutdown (lib); `vary_singleflight_and_cookies` 3.
- **Decisions:** Brotli quality 4, `lgwin` 20. The whole-body cache is removed rather than hash-keyed. Request logs are `debug` (routine success), handler errors `warn`, panics and render failures `error`, so the default `RUST_LOG` stays quiet. `/healthz` is `200` while the token is unexpired, `503` otherwise. The rate-limit counter is informational because it refills by itself. `--healthcheck` prints `ok` on stdout / exits 0, or prints `unhealthy: <reason>` on stderr / exits 1 (clap keeps 2 for bad usage).

### Red-green

| Milestone | Test(s) | Observed red failure |
| --- | --- | --- |
| M3.1 | `compress_response_restores_the_body_when_compression_fails` | `the response body was dropped after a compression error (0 bytes left)` |
| M3.1 | `compress_response_does_not_block_the_async_executor` | `compression ran inline on the async executor` |
| M3.6 | `compress_response_sets_vary_for_compressible_responses` | `no entry found for key "vary"` |
| M3.2 | `comment_prefs_reuse_hides_score_and_awards_for_every_comment` | `comment 0 should show the hidden-score bullet: 42` (the control `..._shows_score_and_awards_by_default` passes before and after) |
| M3.4 | `panic_and_render_errors_failed_render_is_a_500` | `left: 200 right: 500` |
| M3.4 | `panic_and_render_errors_panicking_route_returns_500` | `no HTTP response for /sync-panic: ""` (connection dropped) |
| M3.3 | `static_js_caching_scripts_carry_cache_control` | `missing Cache-Control for /hls.min.js` |
| M3.3 | `static_js_caching_{subreddit,search,user}_loads_hls_only_for_hls_posts` | `versioned HLS scripts expected` (seen by stashing the three listing templates) |
| M3.5 | `request_logs_record_the_route_template_without_personal_data` | `no request log line was emitted for the route template: []` (seen by temporarily dropping the `debug!` call; the first run also showed a logger ordering mistake in the test itself, fixed) |
| M3.5 | `healthz_and_shutdown_report_degraded_when_the_token_expired_or_missing` | `left: "ok" right: "degraded"` (stub assessor always said ok) |
| M3.5 | `healthz_and_shutdown_route_answers_200_then_503` | without the route, `/healthz` falls through to `/:id`, which reaches `canonical_path` and Reddit: the test hung past 60 s and was killed |
| M3.5 | `healthz_and_shutdown_probe_addresses_*`, `healthz_and_shutdown_healthcheck_*` (3) | `left: ["0.0.0.0:8080"] right: ["127.0.0.1:8080"]`; binary exit `Some(2)` with `unexpected argument found` |
| M3.5 | `healthz_and_shutdown_stalled_connection_does_not_block_shutdown` | `shutdown waited for a stalled connection` (seen by temporarily disabling the deadline branch) |
| M3.6 | `vary_singleflight_and_cookies_concurrent_identical_calls_fetch_once` | `left: 10 right: 1` (attribute absent) |
| M3.6 | `vary_singleflight_and_cookies_display_names_use_about_json` | `["/r/rust/hot.json?raw_json=1", "/r/rust/about.json?raw_json=1"]` (stub kept the hot lookup) |
| M3.6 | `vary_singleflight_and_cookies_response_cookies_reads_set_cookie` | `left: [] right: [("theme", "dark"), ("old", "")]` |

### Corrections to the plan

- Plan signature `compress_body_blocking(compressor, Vec<u8>)` became `(compressor, &[u8])` so the original `Bytes` survive for the error path.
- The plan's `build_comment(..., prefs: &Preferences)` is implemented, and `Comment` now holds two booleans instead of a cloned `Preferences`.
- `compress_response` appends `Vary` only for responses that are compression-eligible (text or JSON of 1452 bytes or more), because small and binary responses are never encoded.
- No finding was invalid or already fixed. #53's token-prefix logging half was already done in Slice 1 and is outside this slice.

### Not verified

- `client::json` itself needs live Reddit. The single-flight test proves the `#[cached(..., sync_writes = "by_key")]` attribute combination on a stand-in function, not the call site. A reviewer should confirm the attribute is on `json`.
- Recording the token expiry when a client is installed (`Oauth::record_token_expiry` in `OAUTH_CLIENT` init and `force_refresh_token`) needs live Reddit; `/healthz` is tested with the static set directly.
- The real SIGINT/SIGTERM handler is moved to `shutdown_signal()` unchanged; only the deadline logic is tested through a oneshot.
- `subscriptions_filters` calling `display_name` needs live Reddit; the helper is tested. Multi-subreddit subscriptions (`a+b`) now cost one cached `about.json` per name instead of one hot listing.
- `instance_info` renders still `unwrap()`; a panic there is now caught and answered with 500 rather than fixed.
- No browser check that `defer` on `hls.min.js`/`playHLSVideo.js` keeps playback working: `playHLSVideo.js` runs at the end of the body and deferred scripts run in order, which should be equivalent.
