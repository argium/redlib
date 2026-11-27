# Redlib Audit Remediation — Plan

- **Status:** Ready
- **Author:** Argi
- **Created:** 2026-11-28
- **Language:** Rust (with Askama HTML templates, CSS, vanilla JS, Docker, GitHub Actions)
- **Spec:** None — criteria live in the milestones, sourced from [research.md](./research.md) (#1–#55) and [`intent.md`](./intent.md)

> **Shorthand key:** `S1`–`S7` = Slices 1–7; `M<slice>.<n>` = Milestone `<n>` in `<slice>`; `#N` = ranked item number in [research.md](./research.md). Items marked `[Medium]` belong to Medium-value improvements (#45–#55); they are in scope.

## Summary

Redlib is a Rust web proxy and privacy-focused front-end for Reddit. The static audit ([research.md](./research.md)) confirmed 3 critical bugs, 25 important bugs, 16 high-value improvements, and 11 medium-value improvements across OAuth lifecycle safety, input/access security, performance, domain parsing, accessibility/typography, and container CI.

This plan delivers 7 ordered vertical slices covering #1–#55, ordered by operational and security risk. Agreed requirements come from [research.md](./research.md) and the decisions recorded in [`intent.md`](./intent.md); signatures and tuning values marked "proposed" are implementation guidance that the task lists may refine.

## Prerequisites

- Rust toolchain >= 1.81 (`Cargo.toml` `rust-version = "1.81"`), `cargo-nextest` (optional; `cargo test` works locally).
- **Approved decisions (2026-11-28, see [`intent.md`](./intent.md)):**
  1. **Scope:** All 55 items (#1–#55), including `[Medium]` milestones.
  2. **PR delivery:** The 7-layer stack below.
  3. **#3 HTML sanitisation:** Escape `pulldown_cmark` `Event::Html`/`Event::InlineHtml` directly; no new crate.
  4. **#8 config errors:** Fail startup with a non-zero exit when an existing `redlib.toml` cannot be read or parsed.
  5. **#28 autoplay default:** Wire `REDLIB_DEFAULT_AUTOPLAY_VIDEOS` into `Config`.
- **Local build toolchain (Windows):** `btls-sys` (BoringSSL) needs `cmake` and `nasm` on `PATH`. On this workstation they live at `C:\Program Files (x86)\Microsoft Visual Studio\18\BuildTools\Common7\IDE\CommonExtensions\Microsoft\CMake\CMake\bin` and `%LOCALAPPDATA%\bin\NASM`.

## Verification Rule

Every milestone needs two kinds of evidence before its slice is accepted:

- **Regression unit test:** At least one `#[test]`/`#[tokio::test]` at the owning module's interface. It must fail against the pre-fix code (or a stub reproducing the defect) and pass after the fix. Tests run offline: they use pure functions, fixtures, or loopback sockets, never live Reddit or GitHub.
- **Verification:** The milestone's Acceptance Verification command passes, plus `cargo fmt --check` and `cargo clippy --locked --all-targets` on the slice.

Where a milestone only changes CI or container files (Slice 7), a Rust unit test could only assert source text, which the repository instructions forbid. Those milestones use executable checks (`actionlint`, `docker build`, container healthcheck) as their regression evidence; this exception is revisited when Slice 7 is compiled.

## Local Running and Integration Tests

Redlib is a single-binary `hyper` + `tokio` HTTP server with no database; state lives in memory caches (`cached`, `ArcSwap`) and user cookies.

- **Offline test isolation:** Existing tests call live Reddit and are retried 10× in CI (`NEXTEST_RETRIES: 10`). Slice 1 marks live-network tests `#[ignore = "requires live Reddit"]` and adds regression tests that need no network: pure decision functions (redirect following, upstream error mapping, OAuth retry policy) and loopback TCP sockets (client timeouts, unreachable GitHub). Later slices reuse the same pattern; a full `hyper` mock of Reddit is only added if a milestone cannot be proved this way.
- **Lifecycle commands:**
  - Offline unit + mock integration tests: `cargo test --locked -- --nocapture` (or targeted `cargo test --locked <test_name>`).
  - Lint & format gate: `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings`.
  - Local server smoke run: `cargo run --locked -- --address 127.0.0.1 --port 8080`, readiness via `GET http://127.0.0.1:8080/healthz` (proposed in S3) or `/settings`, teardown via `SIGINT`/`SIGTERM` (verify exit within shutdown deadline).
  - Container smoke check (S7): `docker build -f Dockerfile.alpine -t redlib:test .` and `Dockerfile.ubuntu`, then run container and verify `HEALTHCHECK` reaches `healthy`.
- **Evidence boundary:** Mock-backed tests verify Redlib's contracts against recorded Reddit/GitHub response shapes; they do not prove live Reddit API behaviour (covered separately by manual/scheduled `#[ignore]` runs).

## Pull Request Delivery

- **Mode:** Stack (approved 2026-11-28). Every slice stacks linearly on the files it shares so there are no cross-PR merge conflicts in `src/utils.rs`, `src/config.rs`, `src/settings.rs`, `src/subreddit.rs`, `src/main.rs`, or `templates/*.html`.

| Layer | Base | Includes | Branch |
| --- | --- | --- | --- |
| 1 | `main` | Slice 1 (OAuth lifecycle, client timeouts/redirects, offline test harness) | `fix/oauth-client-reliability` |
| 2 | `fix/oauth-client-reliability` | Slice 2 (Security hardening: HTML injection, redirects, SFW-only, DoS limits, bind/config) | `fix/security-and-input-hardening` |
| 3 | `fix/security-and-input-hardening` | Slice 3 (Server performance, compression, panic boundary, observability & healthcheck) | `perf/server-and-observability` |
| 4 | `perf/server-and-observability` | Slice 4 (Domain, RSS, search, emotes, wiki, config drift & `Preferences` versioning) | `fix/domain-feeds-and-search` |
| 5 | `fix/domain-feeds-and-search` | Slice 5 (Frontend JS, OpenSearch, a11y, keyboard focus, spoilers & markup) | `fix/frontend-a11y-and-js` |
| 6 | `fix/frontend-a11y-and-js` | Slice 6 (Theme contrast, typography, responsive layout & UX polish) | `feat/ux-themes-and-typography` |
| 7 | `feat/ux-themes-and-typography` | Slice 7 (Dockerfiles, CI workflows, dependency pinning & dev ergonomics) | `ci/containers-and-workflows` |

---

## Implementation

### Slice 1 (S1): OAuth Lifecycle Safety, Upstream Client Resilience & Offline Test Harness

Covers **#1, #2, #9, #10, #29 (client timeouts), #36 (test separation & offline harness), #38, #53 (token log part)**.

#### Problem

Redlib proxies Reddit using a shared anonymous OAuth token held in `OAUTH_CLIENT` and guarded by an atomic flag (`OAUTH_IS_ROLLING_OVER`). When a request receives `401 Unauthorized`, `client::json()` awaits `force_refresh_token()` directly inside the HTTP request future. If the browser disconnects mid-refresh or `Oauth::new()` panics (such as `&token[..32]` on a short token or header `to_str().unwrap()`), the flag stays `true` forever and every subsequent request fails until restart. Worse, if Reddit has a 2-minute hiccup during a runtime refresh, `Oauth::new()` calls `std::process::exit(1)` and terminates the running server.

Upstream calls also lack timeouts, redirect-hop caps, clear `403`/`429` error mapping, and safe GitHub error handling (`fetch_commit_info`/`fetch_instances` panic on failure), while automated tests hit live Reddit and mask failures behind 10 CI retries.

#### Solution

Make OAuth construction fallible (`Oauth::try_new() -> Result<Self, String>`) so only startup exits on unrecoverable auth failure; runtime refreshes keep the existing client, log the error, and the token daemon retries after a delay. Protect `OAUTH_IS_ROLLING_OVER` with an RAII drop guard, spawn `401` refreshes onto Tokio rather than awaiting them inside request futures, replace panicking token-slice and header unwraps, and demote token/UUID logs to `trace!`.

Give `CLIENT` a connect timeout and an idle read timeout (an idle timeout resets after each chunk, so long `/vid`, `/hls` and `/img` streams keep flowing), cap redirect recursion at 5 hops and fail on a missing `Location`, map HTTP `403`/`429` clearly without refreshing the token on non-rate-limited empty bodies, and return `Result` from the GitHub proxy helpers so failures become `502`. Mark live-Reddit tests `#[ignore]` so the default suite runs offline (CI removal of `NEXTEST_RETRIES: 10` follows in S7).

#### Slice 1 Milestones

- [x] **M1.1**: `force_refresh_token()` clears `OAUTH_IS_ROLLING_OVER` on success, error, cancellation, or panic; `client::json()` spawns `401` refreshes detached from request futures; `&token[..32]` and OAuth header `to_str().unwrap()` are replaced with non-panicking handling (#1).
- [x] **M1.2**: Runtime OAuth refresh failure never calls `std::process::exit`; startup failure still exits non-zero (#2).
- [x] **M1.3**: `CLIENT` enforces a connect timeout and an idle read timeout (media streams keep working), caps redirects at 5 hops, errors on missing `Location`, maps `403`/`429` cleanly, and skips token refresh on empty bodies that are not rate limited (#9, #29, #38).
- [x] **M1.4**: `/commits.atom` and `/instances.json` return `502` instead of panicking when GitHub fails (#10).
- [x] **M1.5**: The default `cargo test --locked` run makes no live network calls; existing live-Reddit tests are marked `#[ignore]` and run only with `--ignored` (#36, offline test split).
- [x] **M1.6** `[Medium]`: Bearer-token prefix and device UUID logging are demoted from `info!` to `trace!` (#53, log part).

<details>
<summary>⚙️ <b>Technical details</b></summary>

- **Consumes:** None
- **Produces:** See [tasks-slice-1.md](./tasks-slice-1.md) for exact signatures. Later slices rely on:

```rust
// src/oauth.rs
impl Oauth {
    pub(crate) async fn try_new() -> Result<Self, String>;
}
pub async fn force_refresh_token() -> bool; // false only when a refresh was attempted and failed

// src/client.rs
const MAX_REDIRECTS: u8 = 5;
fn build_client_with_timeouts(connect_timeout: Duration, read_timeout: Duration) -> WreqClient;

// src/main.rs
async fn fetch_text(url: &str) -> Result<String, String>;
fn text_response(body: Result<String, String>, content_type: &str) -> Response<Body>;
```

#### State-model verification note

During planning, a disposable Python BFS model checked the proposed OAuth rollover state machine (`rolling_over`, `token_state`, `fail_streak`, `process_alive`: 16 reachable states, 0 deadlocks, 0 unrecoverable states when `RolloverGuard` clears on drop and runtime failures never exit). The script was disposable per `STATE-MODEL.md` and is not committed; real concurrency/drop behaviour is proved by M1.1 and M1.2 tests.

#### Acceptance Verification

| Criterion | Regression unit tests | Boundary | Command |
| --- | --- | --- | --- |
| M1.1 | `rollover_guard_*` (drop, panic, cancelled future), `remember_session_headers_skips_non_ascii_values`, `token_preview_never_panics` | `oauth` module, no network | `cargo test --locked --lib oauth::tests` |
| M1.2 | `authentication_gives_up_with_an_error_after_falling_back_to_web_auth`, `authentication_returns_the_first_success` | `oauth` retry policy, no network | `cargo test --locked --lib oauth::tests::authentication` |
| M1.3 | `client_times_out_when_upstream_stops_responding` (loopback socket), `redirect_path_*`, `upstream_failure_*` | `client` module, loopback only | `cargo test --locked --lib client::tests` |
| M1.4 | `unreachable_update_source_becomes_bad_gateway` (closed loopback port), `fetched_text_is_served_with_its_content_type` | `main` binary, loopback only | `cargo test --locked --bin redlib` |
| M1.5 | Whole default suite passes with live tests ignored | All offline tests | `cargo test --locked` |
| M1.6 | `device_identifiers_are_only_logged_at_trace` (capturing logger) | `oauth` logging | `cargo test --locked --lib oauth::tests::device_identifiers` |

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

</details>

---

### Slice 2 (S2): Security, Access Control, DoS Limits & Startup Binding Hardening

Covers **#3, #4, #5, #6, #7, #8, #13, #33, #45**.

#### Problem

A user viewing a post whose body contains a markdown code fence (` ``` `) receives raw HTML rendered by `pulldown_cmark` straight into `{{ post.body|safe }}`, permitting injected `<meta http-equiv="refresh">` redirects and `<style>` overlays while breaking Reddit spoilers and autolinks. Elsewhere, unvalidated `redirect`/`redir` parameters allow open redirects (`//evil.com` or `https://...`), `REDLIB_SFW_ONLY=on` can be bypassed in listings and RSS by setting a `show_nsfw=on` cookie, and an unbounded `Accept-Encoding` cache plus uncapped POST bodies, uncapped `deflate_decompress`, and ineffective synchronous `timeout(1s, ...)` wrappers allow trivial memory and worker exhaustion.

At startup, `IPV4_ONLY=false` still activates IPv4-only mode and silently replaces `--address 127.0.0.1` with `0.0.0.0`, invalid `redlib.toml` is ignored silently (dropping security settings), server failures exit with code `0`, and scanner traffic to 5–7 character paths triggers up to 6 upstream Reddit HEAD requests per path.

#### Solution

Sanitise post bodies in the `pulldown_cmark` branch (approved: escape `Event::Html`/`Event::InlineHtml` as text) and restore spoiler/autolink handling. Centralise redirect target validation and HTML body escaping inside `utils::redirect()` (rejecting `//`, `/\`, and non-relative URLs) and route all redirect callers through it.

Enforce `REDLIB_SFW_ONLY=on` inside `Preferences::new()`, listing filters, RSS handlers, and metadata-error fallbacks. Bound `determine_compressor` (or drop its cache), enforce streaming body-size limits on `POST /settings` and `/settings/encoded-restore`, cap `deflate_decompress` output with `Read::take`, remove the ineffective synchronous `timeout(1s, ...)` wrappers (or offload decompression/deserialisation to `spawn_blocking`), add `http1_header_read_timeout`, and validate short-link/share-link IDs (`[a-z0-9]`) before calling `canonical_path()`. Fix CLI/env parsing for `IPV4_ONLY`/`IPV6_ONLY` and `--address`, surface `redlib.toml` parse errors loudly (approved: fail startup with a non-zero exit), and exit non-zero on server errors.

#### Slice 2 Milestones

- [ ] **M2.1**: Post bodies containing code fences sanitise raw HTML (approved: escape raw HTML events) and preserve spoilers and `r/`/`u/` autolinks (#3).
- [ ] **M2.2**: `utils::redirect()` rejects off-site/protocol-relative targets (`//evil.com`, `/\evil.com`, `https://...`) and HTML-escapes its fallback body across settings, subscribe/filter, and quarantine routes (#5).
- [ ] **M2.3**: `REDLIB_SFW_ONLY=on` cannot be bypassed via cookies, listings, failed metadata fetches, or RSS (#7).
- [ ] **M2.4**: `Accept-Encoding` parsing cannot grow memory unboundedly; `POST /settings` and `/settings/encoded-restore` enforce streaming byte caps; `deflate_decompress` caps decompressed bytes; ineffective synchronous `timeout(1s, ...)` wrappers are removed or moved to `spawn_blocking`; non-base36 short-link paths return 404 without calling Reddit (#4, #33).
- [ ] **M2.5**: `IPV4_ONLY`/`IPV6_ONLY` parse booleans strictly and never override an explicit `--address`; invalid existing `redlib.toml` handles errors by failing startup non-zero with a diagnostic message; server bind/runtime failures exit with non-zero status (#6, #8, #13).
- [ ] **M2.6** `[Medium]`: Hyper server configures `http1_header_read_timeout` to harden against slowloris connections (#45).

<details>
<summary>⚙️ <b>Technical details</b></summary>

- **Consumes:** S1 offline loopback mock harness
- **Produces (proposed signatures):**

```rust
// src/utils.rs
pub fn sanitize_redirect_path(path: &str) -> String; // returns "/" unless single leading '/', no "//", "/\", CR/LF
pub fn redirect(path: &str) -> Response<Body>;       // validates path + HTML-escapes fallback body
pub fn deflate_decompress_limited(input: &[u8], max_bytes: u64) -> Result<Vec<u8>, String>;

// src/config.rs
impl Config {
    pub fn try_load() -> Result<Self, String>;       // honours approved #8 failure policy
}

// src/main.rs
fn resolve_listener(address: &str, port: &str, ipv4_only: bool, ipv6_only: bool) -> Result<SocketAddr, String>;
```

#### Acceptance Verification

| Criterion | Feature scenarios / assertions | Boundary | Command |
| --- | --- | --- | --- |
| M2.1 | Fixture post with ```` ``` ```` + `<meta http-equiv=refresh>` + `>!spoiler!<` + `r/rust`; assert `<meta` is sanitised and spoiler/link preserved | `utils::parse_post` unit test | `cargo test --locked parse_post_code_fence` |
| M2.2 | Requests to `/settings/update/?redirect=//evil.com`, subscribe `redirect=/evil.com`, and quarantine `redir=https://evil.com`; assert `Location: /` and escaped body | HTTP route tests | `cargo test --locked redirect_safety` |
| M2.3 | With `REDLIB_SFW_ONLY=on` and cookie `show_nsfw=on`, fetch listing, failed-metadata sub, and RSS containing `over_18: true` posts; assert zero NSFW posts returned | `subreddit`/`user`/`settings` tests | `cargo test --locked sfw_only_enforcement` |
| M2.4 | Vary 500 `Accept-Encoding` headers; stream >1 MiB POST body; send deflate bomb expanding >2 MiB; verify no sync `timeout(1s, async { sync })`; request `/login` & `/xmlrpc` and assert 0 mock Reddit calls | `server`/`settings`/`main` tests | `cargo test --locked dos_and_shortlink_limits` |
| M2.5 | Test `IPV4_ONLY=false`, `-4 -a 127.0.0.1`, conflicting `-4 -6`, malformed TOML file, and occupied port non-zero exit | `config` + `main` CLI tests | `cargo test --locked startup_config_and_bind` |
| M2.6 | Open TCP socket and drip partial HTTP/1 headers beyond configured timeout; assert server closes connection | `server` integration test | `cargo test --locked http1_header_timeout` |

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Slice 1 (reuses loopback mock harness for short-link and SFW-only checks)

</details>

---

### Slice 3 (S3): Server Performance, Panic Isolation, Observability & Healthcheck

Covers **#14, #29 (shutdown deadline), #30, #31, #32, #34 (server side), #35, #39, #46, #47, #53 (`ResponseExt::cookies` part)**.

#### Problem

Under concurrent browser traffic, Redlib blocks Tokio async workers by running Brotli compression at maximum quality (`11`) synchronously inside request futures, caches full uncompressed `Vec<u8>` bodies in memory, and drops the response body (returning an empty `200 OK`) if compression errors. Rendering a 1,000-comment thread calls `Preferences::new(req)` once per comment, reparsing the `Cookie` header ~25,000+ times. Every page also fetches an uncached 412 KB `hls.min.js` even when no HLS video is present.

Operationally, handler panics reset TCP connections, template render failures return silent blank `200` pages, there is no request logging or upstream health signal, `/settings` healthchecks cannot detect dead OAuth, and graceful shutdown has no deadline.

#### Solution

Lower Brotli quality (proposed: `4`–`5`, `lgwin` `20`), run compression via `tokio::task::spawn_blocking`, restore uncompressed bytes on error, send `Vary: Accept-Encoding`, and remove or hash-key the `compress_body` `Vec<u8>` cache. Parse cookies once per request and pass `&Preferences` (or the two booleans) into comment building; add single-flight coalescing on `client::json`, iterate comment children by reference, and resolve subreddit display names via `about.json` rather than fetching full hot listings. Cache static JS assets with `?v=<version>`, emit `hls.min.js` only when a page has an HLS video (`alt_url`), and add `defer`.

Wrap route dispatch in `catch_unwind` to return `500`, log and return `500` on Askama render errors, bound graceful shutdown with a deadline, emit privacy-safe structured request logs (method, route template, status, latency—never client IPs), expose `/healthz` reporting OAuth/upstream state, add `redlib --healthcheck`, and fix or remove unused `ResponseExt::cookies()`. (Container/compose healthcheck wiring follows in S7.)

#### Slice 3 Milestones

- [ ] **M3.1**: Compression uses lower Brotli quality (proposed: `4`–`5`) on `spawn_blocking`, removes or hash-keys the whole-body `Vec<u8>` cache, and restores original body bytes on error (#14, #30).
- [ ] **M3.2**: Comment rendering parses cookies once per request without per-comment `Preferences::new()` (#31).
- [ ] **M3.3**: Static JS assets are served with long-lived cache headers + `?v=` version query strings, and `hls.min.js` loads with `defer` only on pages containing HLS video (#35).
- [ ] **M3.4**: Handler panics and Askama render failures log errors and return `500` rather than resetting sockets or returning blank `200`s (#32, #39).
- [ ] **M3.5**: Privacy-safe request logs, `/healthz` endpoint reporting OAuth/upstream health, `redlib --healthcheck` CLI flag, and a bounded graceful-shutdown deadline work end to end (#29 shutdown part, #34 server part).
- [ ] **M3.6** `[Medium]`: Compressed responses include `Vary: Accept-Encoding`; `client::json` coalesces concurrent identical fetches; subreddit display-name lookup avoids fetching full hot listings; comment children iterate by reference; `ResponseExt::cookies()` reads `Set-Cookie` (or is removed if unused) (#46, #47, #53).

<details>
<summary>⚙️ <b>Technical details</b></summary>

- **Consumes:** S1 OAuth health state & mock harness; S2 `determine_compressor` and `server.rs` changes
- **Produces (proposed signatures):**

```rust
// src/server.rs
pub fn compress_body_blocking(compressor: CompressionType, body_bytes: Vec<u8>) -> Result<Vec<u8>, String>;

// src/main.rs
// CLI flag: `--healthcheck` (queries local http://127.0.0.1:<port>/healthz, exits 0 on 200 else 1)
// Proposed GET /healthz JSON: {"status":"ok"|"degraded","oauth_ready":bool,"ratelimit_remaining":u16} (200 or 503)

// src/post.rs
fn build_comment(..., prefs: &Preferences) -> Comment;
```

#### Acceptance Verification

| Criterion | Feature scenarios / assertions | Boundary | Command |
| --- | --- | --- | --- |
| M3.1 | `compress_response` tests verify valid Brotli output without whole-body `Vec<u8>` key bloat, and simulated compressor error restores original body | `server` unit tests | `cargo test --locked compress_response` |
| M3.2 | Fixture thread with 200 comments renders identically without per-comment `Preferences::new` calls | `post` unit test | `cargo test --locked comment_prefs_reuse` |
| M3.3 | Route tests for `/hls.min.js` (`Cache-Control` present) and rendered subreddit/search/user pages with/without `alt_url` | `main` + template render tests | `cargo test --locked static_js_caching` |
| M3.4 | Test route that panics returns HTTP 500; failing template render returns HTTP 500 | `server` + `utils` tests | `cargo test --locked panic_and_render_errors` |
| M3.5 | Start server against mock OAuth; verify `GET /healthz` returns 200/503 on healthy/unhealthy OAuth, `--healthcheck` exits 0/1, and shutdown exits within deadline with a stalled socket | Integration test | `cargo test --locked healthz_and_shutdown` |
| M3.6 | Assert `Vary: Accept-Encoding`; 10 concurrent identical `client::json` calls issue 1 mock upstream call; display-name lookup hits `about.json` not `/hot.json`; `ResponseExt::cookies` reads `Set-Cookie` | `server` + `client` + `subreddit` tests | `cargo test --locked vary_singleflight_and_cookies` |

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Slice 1 (OAuth state & mock harness), Slice 2 (`src/server.rs` `determine_compressor` & header-timeout changes)

</details>

---

### Slice 4 (S4): Domain Parsing, Feeds, Search, Emotes & Settings Versioning

Covers **#16, #17, #18, #19, #20, #27, #28, #48 (URL-encoding part), #49, #50**.

#### Problem

Several core Reddit views silently corrupt or drop user content: `/r/:sub/w/*page` redirects to `/r/<sub>/wiki/` because it reads the wrong route param; `REDLIB_ENABLE_RSS=off` still enables RSS while `subreddit::rss` double-escapes titles and `user::rss` decodes raw text into markup without a channel `<link>`; search queries with `&`, `#`, `+`, or `%` lose their tail on subreddit lookup and pagination; comment search matches raw HTML tags (`href`, `class`, `md`); `rewrite_emotes` mispairs IDs and links whenever `media_metadata` contains GIFs; invalid gallery items render broken `<img>` tags; `/info.txt` prints four config fields out of order; `app.json` and `REDLIB_DEFAULT_AUTOPLAY_VIDEOS` have drifted from `Config`; and `Preferences` export declares `#[revisioned(revision = 1)]` yet serialises with plain `bincode`, breaking future schema additions.

#### Solution

Fix each domain handler and parser with offline JSON fixtures covering every defect: read `r.param("page")` on the wiki shortcut; gate both RSS endpoints with `utils::enable_rss()`, fetch with `raw_json=1` without `decode_html`, and set the `user::rss` channel `<link>`; percent-encode search queries, flair-search links, and wall/NSFW redirect query params while removing manual `&quot;` replacement; filter comment search against raw `data["body"]`; iterate `media_metadata.as_object()` entries directly in `rewrite_emotes` and skip invalid gallery entries (`status != "valid"` or missing `s`); align `/info.txt` format arguments; fix `app.json` keys and wire `REDLIB_DEFAULT_AUTOPLAY_VIDEOS` into `Config`; and switch `Preferences` export/import to versioned serialisation while preserving compatibility with existing v1 exports.

#### Slice 4 Milestones

- [ ] **M4.1**: `/r/:sub/w/*page` preserves `*page` in the redirect; `/info.txt` prints all fields under matching labels; `app.json` matches `Config` keys and `REDLIB_DEFAULT_AUTOPLAY_VIDEOS` is read into `Config` (#20, #27, #28).
- [ ] **M4.2**: RSS respects `REDLIB_ENABLE_RSS=on`, uses `raw_json=1` without unsafe `decode_html`, avoids double-escaping, and emits a valid RSS 2.0 `<link>` on user feeds (#16).
- [ ] **M4.3**: Search queries, subreddit search, and search pagination percent-encode `&`, `#`, `+`, and `%` without `&quot;` double-escaping, and comment search matches text rather than HTML markup (#17, #18).
- [ ] **M4.4**: `rewrite_emotes` pairs each emote's own ID, URL, and height accurately in mixed GIF/emote comments (#19).
- [ ] **M4.5** `[Medium]`: Flair-search and wall/NSFW redirect links are URL-encoded; `GalleryMedia::parse` skips invalid/missing items; `Preferences` export/import uses versioned serialisation while remaining backward-compatible with `KNOWN_GOOD_CONFIGS` v1 payloads (#48 URL-encoding part, #49, #50).

<details>
<summary>⚙️ <b>Technical details</b></summary>

- **Consumes:** S1 offline mock harness; S2 `utils::redirect()`, `sfw_only()` RSS/Preferences hooks, and `config.rs`; S3 `post.rs` comment builder
- **Produces (proposed signatures):**

```rust
// src/config.rs (if #28 wires default_autoplay_videos)
pub struct Config {
    // ... existing fields ...
    pub(crate) default_autoplay_videos: Option<String>,
}

// src/search.rs
pub struct SearchParams {
    pub q: String,      // raw text (Askama HTML-escapes automatically)
    pub q_enc: String,  // percent-encoded for href query strings
    // ... existing fields ...
}

// src/utils.rs
// Preferences versioned export/import with v1 bincode fallback
```

#### Acceptance Verification

| Criterion | Feature scenarios / assertions | Boundary | Command |
| --- | --- | --- | --- |
| M4.1 | Route test `/r/rust/w/faq/index` -> `/r/rust/wiki/faq/index`; `/info.txt` field assertions; `Config` + `app.json` key checks | `main` + `instance_info` + `config` | `cargo test --locked wiki_info_and_config` |
| M4.2 | `REDLIB_ENABLE_RSS=off` returns disabled; fixture subreddit & user feeds check XML validity, `<link>`, single-escaped `&`, and non-decoded `<img` text | `subreddit::rss` + `user::rss` | `cargo test --locked rss_feeds` |
| M4.3 | Query `C++ & AT&T #1 100%` through search and pagination hrefs; comment search for `href` does not match link-only comments | `search` + `post` + templates | `cargo test --locked search_encoding_and_comments` |
| M4.4 | Mixed Giphy + multi-emote `media_metadata` fixture asserts exact ID/URL/height pairing | `utils` unit tests | `cargo test --locked rewrite_emotes` |
| M4.5 | Flair `C# & 100%` and redirect URL encoding checks; failed gallery item omitted; round-trip versioned `Preferences` + `KNOWN_GOOD_CONFIGS` v1 decode | `utils` + `settings` + templates | `cargo test --locked flair_gallery_and_prefs_versioning` |

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Slice 1 (mock harness), Slice 2 (shared `subreddit.rs`/`user.rs` RSS SFW checks, `config.rs`, `utils::redirect()`), Slice 3 (`post.rs` comment builder)

</details>

---

### Slice 5 (S5): Frontend JS Reliability, OpenSearch, Keyboard Accessibility & Valid HTML

Covers **#15, #21, #25, #26, #42 (copy.js & gallery `<p>` parts), #43, #44, #48 (HTML markup part), #52**.

#### Problem

Several interactive features break silently in browsers: `check_update.js` throws `TypeError` on outdated instances because `#error-446` doesn't exist; `playHLSVideo.js` retries fatal HLS errors in an infinite loop with no backoff; `opensearch.xml` hardcodes `https://localhost:8080`; `highlighted.js` is skipped on image and video posts; `copy.js` fails silently on plain HTTP / `.onion` instances; and Open Graph tags emit relative URLs that link-preview crawlers ignore.

Keyboard and screen-reader users face direct blockers: mobile `#search:focus` removes its outline, `overflow-y: hidden` clips focus rings on sort/listing/pagination tabs, spoilers and blurred NSFW/spoiler media reveal only on `:hover`, duplicate `#video_quality` (placed inside `<head>`) and missing `id`s on four settings controls disconnect `<label>`s, icon-only controls lack accessible names, and there is no skip link or page `<h1>` on feeds.

#### Solution

Fix the JS scripts: cap HLS fatal retries (proposed: 3 attempts with exponential backoff) before destroying the instance, showing an error state, and falling back to MP4; guard `#error-446` removal in `check_update.js`; add `execCommand` fallback and visual "Copied" feedback in `copy.js`; and move `highlighted.js` outside the post-type `if` chain. Render `opensearch.xml` dynamically from `REDLIB_FULL_URL` or the request `Host`, and prefix `og:image`/`twitter:image`/`og:video` with an absolute base URL when available.

Fix HTML validity and keyboard accessibility: move `video_quality` out of `<head>` onto a `data-video-quality` attribute on `<body>`; add missing `id`s on settings controls, accessible names on search/nav/HLS controls, a "Skip to content" link, and proper `<h1>` landmarks; restore visible focus outlines on mobile search and tab bars; enable `:focus-within` / explicit reveal for spoilers and blurred media; and clean up unquoted `href`, unclosed gallery `<p>`, duplicate IDs (`#search`, `#sort`, `#update-status`), `<legend>` outside `<fieldset>`, and `<button>` inside `<a>`.

#### Slice 5 Milestones

- [ ] **M5.1**: `playHLSVideo.js` caps fatal retries with backoff and falls back cleanly with an error state; `check_update.js` reports commit lag without throwing; `copy.js` works on plain HTTP and shows visual feedback (#15, #26, #42 copy part).
- [ ] **M5.2**: `/opensearch.xml` uses the instance's actual host/base URL, and `og:image`/`twitter:image`/`og:video` emit absolute URLs when `REDLIB_FULL_URL` or `Host` is known (#25, #43).
- [ ] **M5.3**: All form controls, icon buttons, mobile nav links, and HLS quality selectors have accessible names; no `<div id="video_quality">` in `<head>`; unclosed gallery `<p>` is closed (#21, #42 gallery `<p>` part).
- [ ] **M5.4**: Focus rings remain visible on mobile search, sort/listing tabs, and pagination; spoilers and blurred media are revealable by keyboard (`:focus-within` / focusable control); skip-to-content link and feed `<h1>` landmarks are present (#21, #44).
- [ ] **M5.5** `[Medium]`: `highlighted.js` loads on image and video single-thread posts; unquoted `href`, duplicate IDs (`#search`, `#sort`, `#update-status`), `<legend>` outside `<fieldset>`, and `<button>` inside `<a>` are fixed (#48 markup part, #52).

<details>
<summary>⚙️ <b>Technical details</b></summary>

- **Consumes:** S3 `?v=` static JS caching and `defer` `hls.min.js` templates; S4 template updates
- **Produces (proposed signatures & markup):**

```rust
// src/main.rs
// Route: GET /opensearch.xml -> dynamic handler reading REDLIB_FULL_URL or Host header
// src/utils.rs
pub fn to_absolute_url(path: &str) -> String;
```

```html
<!-- templates/base.html -->
<body data-video-quality="{{ prefs.video_quality }}" ...>
  <a class="skip-link" href="#main-content">Skip to content</a>
  ...
  <main id="main-content">
```

#### Acceptance Verification

| Criterion | Feature scenarios / assertions | Boundary | Command |
| --- | --- | --- | --- |
| M5.1 | JS unit checks on `playHLSVideo.js` retry cap/backoff, `check_update.js` outdated path, and `copy.js` HTTP fallback + feedback | JS unit checks / browser smoke | Node or browser verification script on `static/*.js` |
| M5.2 | HTTP test for `/opensearch.xml` with `Host: redlib.example` asserts no `localhost:8080`; post render test with `REDLIB_FULL_URL` asserts `https://...` in `og:image` and `og:video` | Route + template tests | `cargo test --locked opensearch_and_og` |
| M5.3 | Template render tests across `base.html`, `settings.html`, `post.html`, `utils.html` asserting `<label for>` matches, accessible names, closed gallery `<p>`, and valid `<head>` | Template HTML assertions | `cargo test --locked template_a11y_and_html` |
| M5.4 | CSS/template checks confirming skip link, `:focus-within` on `.post_blurred`/`.md-spoiler-text`, no `#search:focus { outline: 0 }`, and non-clipped tab focus | Static CSS + keyboard walkthrough | `cargo test --locked a11y_markup` + manual keyboard tab check |
| M5.5 | Template render tests asserting `highlighted.js` on image/video single-thread posts, unique IDs, quoted `href`, and valid `<fieldset>`/`<a>` nesting | Template HTML assertions | `cargo test --locked html_markup_and_highlighted` |

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Slice 3 (`main.rs` static JS routes and `hls.min.js` `defer` templates), Slice 4 (shared `templates/*.html` edits)

</details>

---

### Slice 6 (S6): Theme Contrast, Typography, Responsive Layout & Reading UX

Covers **#22, #23, #24, #40, #41, #42 (error/empty/settings-save/restore-errors/gallery UX parts), #51, #54**.

#### Problem

Reading long threads and browsing on mobile suffer from compounding visual defects: visited post titles drop to 1.2:1–2.1:1 contrast in five themes; 40–50% opacity metadata and accent buttons fall below WCAG AA (including default `dark` and `light`); `.md` links lack underlines; all type is `px`-locked with no body `line-height` (~1.21) and a 325 KB `Inter.var.woff2` that causes FOIT; settings rows overlap when labels wrap; the "leave Redlib" popup clips its buttons on landscape phones; and `fixed_navbar` overlaps mobile pages without manual margin patches.

On top of those bugs, comment thread lines are nearly invisible and non-collapsible, `<blockquote class="replies">` announces every reply depth as a quotation, deep mobile threads crush text to ~20 characters wide, mobile subreddits open with the expanded sidebar above posts, wide mode stretches prose past 200 characters per line, dark themes show light native scrollbars under a light OS, and error/empty/gallery/settings pages lack clear recovery actions or inline error feedback.

#### Solution

Audit and update all 18 theme stylesheets so `--visited`, muted text, and `--accent` / button foreground tokens meet WCAG AA (>= 4.5:1) and every theme declares `color-scheme: light` or `dark`. Convert typography to `rem`, add `line-height: 1.55` on prose/comments, add `font-display: swap` with a system-UI fallback stack, upgrade monospace to `ui-monospace, "SF Mono", Menlo, Consolas, monospace` with inline-code padding, underline `.md` links, scale single-post `h1` above markdown headings, and cap `.wide` prose measure at ~70 ch.

Fix responsive layouts: auto-height `.prefs-group`, scrollable `.popup-inner` (`max-height: 90dvh`), CSS-variable mobile `fixed_navbar` offset, >= 44 px touch targets on touch devices, and collapsed subreddit `<details>` by default on mobile (or ordered after posts). Upgrade comment threads (higher-contrast collapsible `.line`, `<div role="group">` instead of `<blockquote>`, capped mobile indent depth), respect `prefers-reduced-motion` (including video autoplay) and `forced-colors`, replace `display: none` upvote labels with `.sr-only`, humanise theme names with a "Reset to defaults" button, omit empty gallery `<p>` tags when captions are absent, add gallery "1 of N" counters + post-title alt text, add error retry and empty-state recovery links, and render settings save confirmation and restore errors inline on `/settings`.

#### Slice 6 Milestones

- [ ] **M6.1**: All 18 themes define explicit `--visited`, muted text, and accent/button contrast >= 4.5:1; `.md` links are underlined (#22).
- [ ] **M6.2**: Typography uses `rem`, readable body/comment `line-height`, `font-display: swap`, clear post-title vs. markdown heading hierarchy, and ~70 ch max prose measure in wide mode (#24, #41).
- [ ] **M6.3**: Settings rows do not overlap, exit popup scrolls on short viewports, `fixed_navbar` clears all mobile pages, touch targets meet >= 44 px, and mobile feed shows posts before expanded sidebar (#23, #41).
- [ ] **M6.4**: Comment threads use `<div role="group">`, higher-contrast collapsible guide lines, and capped mobile indent depth; error pages offer retry, empty feeds offer recovery cues, galleries omit empty `<p>` captions and show "1 of N" + post-title alt text, and settings save/restore show inline confirmation and user-friendly errors (#40, #42).
- [ ] **M6.5** `[Medium]`: All 18 themes declare `color-scheme`; `prefers-reduced-motion` (including video autoplay) and `forced-colors` are respected; monospace stack + inline-code padding, `.sr-only` upvote labels, humanised theme names, and "Reset to defaults" are in place (#51, #54).

<details>
<summary>⚙️ <b>Technical details</b></summary>

- **Consumes:** S2 `src/settings.rs` & `src/utils.rs` changes; S5 accessible markup and skip link in templates
- **Produces:** Updated `static/style.css`, `static/themes/*.css`, `templates/comment.html`, `templates/error.html`, `templates/subreddit.html`, `templates/search.html`, `templates/settings.html`, `templates/utils.html`, inline feedback handling in `src/settings.rs`, and theme-label helper in `src/utils.rs`.

#### Acceptance Verification

| Criterion | Feature scenarios / assertions | Boundary | Command |
| --- | --- | --- | --- |
| M6.1 | Proposed contrast check script parses `static/style.css` + all 18 `static/themes/*.css` and asserts >= 4.5:1 for text, `--visited`, muted metadata, and `--foreground` on `--accent` | Static CSS contrast check | Contrast verification script over `static/themes/*.css` |
| M6.2 | CSS inspection confirms `rem` sizing, prose `line-height`, `font-display: swap`, heading scale, and `max-width: 70ch` on `.wide` prose | Static CSS + visual browser check | Manual desktop (1920 px wide mode) + 200% zoom check |
| M6.3 | Viewport checks at 375×667, 667×360 (landscape popup), and 800 px with `fixed_navbar=on` across `/settings`, `/error`, `/wiki`, `/r/rust` | Browser responsive check | Manual browser viewport check (no overflow/clipping/overlap) |
| M6.4 | Template/route tests assert `<div role="group">` in `comment.html`, gallery `"1 of N"` + post-title alt + omitted empty `<p>`, retry/empty links, and inline `/settings` save confirmation + restore error display | Route + template tests | `cargo test --locked ux_templates_and_settings_feedback` |
| M6.5 | Static CSS checks verify `color-scheme` in all 18 themes, `prefers-reduced-motion`/`forced-colors` rules, monospace stack; template tests verify `.sr-only` score labels, humanised theme names, and reset action | Static CSS + template tests | `cargo test --locked themes_and_sr_labels` |

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Slice 5 (shared templates and `static/style.css`), Slice 2 (`src/settings.rs` changes, transitive via stack)

</details>

---

### Slice 7 (S7): Container Pipeline, CI Hardening, Supply-Chain Pinning & Dev Ergonomics

Covers **#11, #12, #34 (container/compose healthcheck wiring), #36 (CI retry removal), #37, #55**.

#### Problem

Every push to `main` runs two redundant musl build workflows (`build-artifacts.yaml` and `main-rust.yml`), uploads to a `draft: true` release, and triggers `main-docker.yml` with no `conclusion == 'success'` guard—meaning failed builds or new commits publish container images tagged `sha-<commit>` and `latest` containing the previous published release binary. Meanwhile, the root `Dockerfile` has no default `TARGET`, uses EOL `alpine:3.19`, pipes an unverified `curl -L` (no `-f`) into `tar` (breaking `compose.dev.yaml` and `heroku.yml`), `Dockerfile.ubuntu` runs a `wget` healthcheck on an image without `wget`, and `compose.yaml` polls `/settings` every 5 minutes without `start_period`.

In CI and local dev, `pull-request.yml` runs with `NEXTEST_RETRIES: 10`, without `--locked`, `rust-cache`, `clippy --all-targets`, or any container/musl smoke build, uses `actions/checkout@v3`, and `Cargo.toml` leaves RC crates (`wreq`, `wreq-util`) caret-unpinned. `.dockerignore` ships `.git` into build contexts, `.devcontainer` runs `cargo build` on every start, `flake.nix` floats `rust-bin.stable.latest` with `doCheck = false`, and there is no `justfile`.

#### Solution

Consolidate the duplicate release workflows, gate `main-docker.yml` on `github.event.workflow_run.conclusion == 'success'`, and build release containers from the workflow's own build artifact (with SHA-256 verification) or from source rather than `releases/latest`. Fix `Dockerfile`, `Dockerfile.alpine`, `Dockerfile.ubuntu`, `compose.yaml`, `compose.dev.yaml`, and `heroku.yml` to share a supported pinned Alpine base, build local source in dev/Heroku, and switch container `HEALTHCHECK`s to `redlib --healthcheck` (from S3) with appropriate `start_period` and interval.

Update `pull-request.yml` to drop `NEXTEST_RETRIES: 10`, enforce `--locked`, add `Swatinem/rust-cache`, `clippy --all-targets`, `actions/checkout@v4`, `cargo-audit`/`cargo-deny`, and a PR musl/container smoke build. Pin `wreq` and `wreq-util` with `=` in `Cargo.toml`, add `.git` to `.dockerignore`, stop devcontainer auto-builds, pin `flake.nix` to Rust `1.81` with checks enabled, and add a `justfile` with `fmt`, `clippy`, `test`, and `lint` recipes.

#### Slice 7 Milestones

- [ ] **M7.1**: `main-docker.yml` runs only on successful upstream builds and packages the exact commit's verified binary (with SHA-256 check), eliminating duplicate musl release builds (#11, #37).
- [ ] **M7.2**: `compose.yaml`, `compose.dev.yaml`, `heroku.yml`, `Dockerfile`, `Dockerfile.alpine`, and `Dockerfile.ubuntu` all build cleanly on supported base images and use `redlib --healthcheck` with sensible intervals/`start_period` (#12, #34 container wiring).
- [ ] **M7.3**: `pull-request.yml` drops `NEXTEST_RETRIES: 10`, enforces `--locked`, `clippy --all-targets`, `rust-cache`, `checkout@v4`, dependency audit, and a PR container/musl smoke build; RC crates are exact-pinned in `Cargo.toml` (#36 CI part, #37).
- [ ] **M7.4** `[Medium]`: `.dockerignore` excludes `.git`, `.devcontainer/devcontainer.json` omits `postStartCommand` full builds, `flake.nix` pins Rust `1.81` with checks enabled, and `justfile` defines `fmt`, `clippy`, `test`, and `lint` recipes (#55).

<details>
<summary>⚙️ <b>Technical details</b></summary>

- **Consumes:** S1 offline test separation (`#[ignore]` live tests); S3 `redlib --healthcheck` CLI flag
- **Produces:** Updated `.github/workflows/*.yaml`, `Dockerfile`, `Dockerfile.alpine`, `Dockerfile.ubuntu`, `compose.yaml`, `compose.dev.yaml`, `heroku.yml`, `.dockerignore`, `.devcontainer/devcontainer.json`, `flake.nix`, `Cargo.toml`, `justfile`.

#### Acceptance Verification

| Criterion | Feature scenarios / assertions | Boundary | Command |
| --- | --- | --- | --- |
| M7.1 | Workflow YAML lint/inspection confirms `conclusion == 'success'` gate, single musl build path, and checksum-verified artifact handoff | CI workflows | `actionlint` (if installed) + YAML validation |
| M7.2 | Build `Dockerfile.alpine` and `Dockerfile.ubuntu` locally; run container and verify `HEALTHCHECK` (`redlib --healthcheck`) reaches `healthy`; inspect `compose.yaml`/`compose.dev.yaml`/`heroku.yml` | Docker runtime | `docker build -f Dockerfile.alpine . && docker build -f Dockerfile.ubuntu .` |
| M7.3 | Verify `pull-request.yml` has no `NEXTEST_RETRIES: 10`, enforces `--locked`, and `Cargo.toml` uses exact `=` pins for RC crates; run local lint + test | Cargo + CI | `cargo fmt --check && cargo clippy --locked --all-targets -- -D warnings && cargo test --locked` |
| M7.4 | Verify `.git` excluded from `.dockerignore`, `flake.nix` pins `1.81` with checks enabled, and `just lint` / `just test` execute cleanly | Repo tooling | `just --list && just lint` |

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Slice 1 (offline test suite so CI retries can be dropped), Slice 3 (`redlib --healthcheck` for Docker/compose healthchecks), Slice 6 (linear stack base)

</details>

---

### Documentation

- [ ] No living `docs/specs/<capability>.feature` files exist in this repository today; acceptance is proved via Rust unit/mock-integration tests, CSS contrast checks, and container smoke checks per slice.
- [ ] Update `README.md` and `.env.example` for `/healthz` + `--healthcheck` (#34), `IPV4_ONLY`/`IPV6_ONLY` boolean parsing (#6), `redlib.toml` error behaviour (fail fast), and `REDLIB_DEFAULT_AUTOPLAY_VIDEOS` (now configurable).

## File Changes

| File | Change |
| --- | --- |
| `src/oauth.rs` | `RolloverGuard` RAII drop guard, `try_new() -> Result`, non-exiting runtime refresh, safe slice/header handling, `trace!` token logs (S1) |
| `src/client.rs` | Spawn `401` refresh, connect/request/streaming timeouts, 5-hop redirect cap + missing `Location` error, `403`/`429` mapping, single-flight `json()`, test-only base URL overrides (S1, S3) |
| `src/main.rs` | Safe GitHub proxy handlers, boolean `IPV4_ONLY`/`IPV6_ONLY` + `--address` validation, non-zero exit code, `/r/:sub/w/*page` fix, short-link validation, static JS caching, `/healthz`, `--healthcheck`, dynamic `/opensearch.xml` (S1–S5) |
| `src/server.rs` | Bound/remove `determine_compressor` cache, `spawn_blocking` Brotli quality 4–5 + cache cleanup, restore body on compress error, `Vary: Accept-Encoding`, `http1_header_read_timeout`, `catch_unwind` 500 handler, request logging, shutdown deadline, `ResponseExt::cookies` fix (S2, S3) |
| `src/config.rs` | Invalid `redlib.toml` handling (fail fast, approved), `default_autoplay_videos` (approved) (S2, S4) |
| `src/utils.rs` | Sanitise `pulldown_cmark` HTML (escape, approved) + spoilers/autolinks, shared `sanitize_redirect_path` + HTML-escaped `redirect()`, `deflate_decompress_limited`, `sfw_only()` enforcement in `Preferences::new()`, 500 on template error, fix `rewrite_emotes`, filter invalid gallery items, versioned `Preferences` serialisation, humanised theme labels (S2–S6) |
| `src/settings.rs` | Streaming POST body size caps, remove/offload sync timeouts, `sfw_only()` cookie enforcement, inline save/restore feedback, reset defaults (S2, S4, S6) |
| `src/post.rs` | Pass `&Preferences` into comment builder, iterate children by reference, search raw `data["body"]` (S3, S4) |
| `src/subreddit.rs`, `src/user.rs` | Enforce `sfw_only()` & `enable_rss()`, fix RSS escaping/`raw_json=1`/channel `<link>`, route redirects through `utils::redirect()`, resolve display names without hot-listing fetch (S2–S4) |
| `src/search.rs`, `src/instance_info.rs` | Percent-encode `q` + drop manual `&quot;` replace; align `/info.txt` arguments (S4) |
| `static/*.js`, `static/opensearch.xml` | HLS retry cap + backoff, `check_update.js` null guard, `copy.js` HTTP fallback + feedback, dynamic OpenSearch (S5) |
| `static/style.css`, `static/themes/*.css` | Theme contrast + `color-scheme`, `rem` + `line-height` + `font-display: swap` + monospace + `70ch` measure, focus rings, keyboard spoilers/blur, responsive layout, `prefers-reduced-motion`/`forced-colors`, comment thread hierarchy (S5, S6) |
| `templates/*.html` | Fix `<head>` `video_quality`, control IDs & `aria-label`s, skip link, `<h1>`, absolute OG URLs, `highlighted.js` placement, conditional `defer` `hls.min.js`, `<div role="group">` comments, gallery counters/alt/caption `<p>`, error/empty/settings UX, valid HTML (S3–S6) |
| `Dockerfile*`, `compose*.yaml`, `heroku.yml`, `.dockerignore`, `.github/workflows/*.yaml`, `Cargo.toml`, `flake.nix`, `.devcontainer/devcontainer.json`, `app.json`, `.env.example`, `README.md`, `justfile` | Fix container builds & `redlib --healthcheck` wiring, consolidate and gate CI workflows, drop `NEXTEST_RETRIES: 10`, pin RC crates and Nix toolchain, fix config drift, add `justfile` (S4, S7) |

## Notes

- **Intent coverage review (2026-11-28):** A fresh read-only subagent compared `research.md` (#1–#55), `intent.md`, `plan.md`, and `README.md`. All 55 items were covered; the reviewer caught dropped sub-bullets (`#1` slice/header unwraps, `#4` sync timeouts, `#29` streaming media timeout, `#30` `compress_body` cache, `#34` `compose.yaml` healthcheck wiring, `#36` CI retry removal ownership, `#42` empty gallery `<p>` and inline restore errors, `#47` hot-listing display-name fetch, `#51` `forced-colors` + reduced-motion video autoplay, `#55` `justfile` recipe alignment), cross-slice PR base overlaps (S4/S2, S5/S3, S6/S2), and pre-decided open questions. All findings were resolved in this revision by splitting `[Medium]` milestones explicitly, making the PR stack linear across overlapping files, adding missing sub-bullet milestones, and labelling proposed signatures/defaults distinctly from agreed requirements.
- **Trade-offs & exclusions:**
  - *`hyper 0.14` migration:* Upgrading `hyper 0.14` -> `1.x` is a large framework rewrite not requested in `research.md` and is intentionally excluded from this plan.
  - *Workspace location:* Moved from `.plans/` to the skill-prescribed `projects/2026-11-28-redlib-audit-remediation/` on 2026-11-28 at the user's request.
