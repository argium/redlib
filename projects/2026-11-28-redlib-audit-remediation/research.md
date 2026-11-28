# Redlib Audit & Improvements (Ranked by Severity)

Static, read-only audit of Redlib (`0.40.0`) across the HTTP server, OAuth and Reddit client, domain parsing, templates, client-side JS, CSS/themes/accessibility/typography, container builds, and CI. Nothing was built, run, or rendered in a browser; every finding below is verified from source code and ranked from highest to lowest severity and impact.

---

## Critical Bugs

1. **Cancelled or panicked token refresh permanently locks `OAUTH_IS_ROLLING_OVER`**
   - **Where:** `src/oauth.rs:182-190`, `src/client.rs:485`, `src/oauth.rs:285`
   - **What happens:** `force_refresh_token()` sets `OAUTH_IS_ROLLING_OVER` to `true`, awaits `Oauth::new()`, and only clears the flag at the end with no drop guard. `client::json()` awaits `force_refresh_token()` directly inside the HTTP request future on a `401 Unauthorized` response. If the client disconnects while the refresh is in flight (hyper cancels the handler future), or if `Oauth::new()` panics (for example `&token[..32]` at `src/oauth.rs:285` on a short token, or header `to_str().unwrap()`), the flag stays `true` forever. Every subsequent `force_refresh_token()` returns immediately, `token_daemon` becomes a no-op, and all requests fail with `"OAuth token has expired"` until the process is restarted.
   - **Fix:** Clear `OAUTH_IS_ROLLING_OVER` in an RAII drop guard, spawn the `client.rs:485` refresh via `tokio::spawn` like the other call sites, and replace the panicking slice/header unwraps with safe bounds checks.

2. **Transient upstream outage during token refresh terminates the server**
   - **Where:** `src/oauth.rs:109-112`, `src/oauth.rs:189`
   - **What happens:** `Oauth::new()` calls `std::process::exit(1)` after 10 failures (~100 s). Runtime token refreshes in `force_refresh_token()` call the same function, so a brief Reddit or network blip at refresh time kills an otherwise healthy instance.
   - **Fix:** Return `Result<Self, AuthError>` from `Oauth::new()`. Only exit on startup failure; during runtime refreshes, keep the existing client, log the failure, and retry with backoff.

3. **Stored HTML injection and broken spoiler/autolink parsing in post bodies with code fences**
   - **Where:** `src/utils.rs:813-817`, `templates/utils.html:159`
   - **What happens:** When `selftext` contains ```` ``` ````, `parse_post()` re-renders it with `pulldown_cmark::html::push_html` instead of using Reddit's `selftext_html`, then emits the output unescaped via `{{ post.body|safe }}`. `pulldown_cmark` passes raw HTML through verbatim. Although the CSP (`src/main.rs:212`) blocks scripts and inline event handlers, raw `<meta http-equiv="refresh">`, `<style>` overlays, and injected markup still execute. The same branch also drops Reddit-specific markdown (`>!spoiler!<` renders as plain text, and `r/` / `u/` autolinks disappear).
   - **Fix:** Escape or strip `Event::Html` and `Event::InlineHtml` (or sanitise with `ammonia`) before emitting HTML, and preserve spoiler/autolink handling.

---

## Important Bugs

4. **Memory exhaustion via unbounded `Accept-Encoding` cache, uncapped POST bodies, and decompression bomb**
   - **Where:** `src/server.rs:443-444`, `src/settings.rs:75-79`, `src/settings.rs:276-298`, `src/utils.rs:752-757`
   - **What happens:**
     - `determine_compressor(accept_encoding: String)` uses `#[cached]` with no size bound (`UnboundCache`), keyed on attacker-controlled `Accept-Encoding` headers for any response >= 1452 bytes.
     - `/settings/encoded-restore` reads the full POST body with `hyper::body::to_bytes` *before* checking the 1 MiB limit, and `POST /settings` has no size cap at all.
     - `deflate_decompress()` calls `read_to_end` with no output cap (classic decompression bomb). The `timeout(1s, async { ... })` wrappers at `src/settings.rs:291,295` wrap synchronous CPU work with no `.await`, so they cannot interrupt decompression or deserialisation and block Tokio worker threads.
   - **Fix:** Bound `determine_compressor` (or remove its cache—parsing is cheap), enforce body limits while streaming (e.g. `http_body::Limited` / chunk accumulator), wrap `Decoder` in `Read::take(MAX_DECOMPRESSED_BYTES)`, and remove the ineffective timeouts (or move work to `spawn_blocking`).

5. **Open redirects on settings, subscribe/filter, and quarantine endpoints**
   - **Where:** `src/settings.rs:119-131`, `src/subreddit.rs:216-217`, `src/subreddit.rs:352-358`, `src/utils.rs:1304-1311`
   - **What happens:**
     - `GET /settings/update/?redirect=//evil.com` passes `value.starts_with('/')` and redirects off-site while also mutating preference cookies.
     - `POST` subscribe/filter handlers format `format!("/{redirect_path}")`, so `redirect=/evil.com` becomes `Location: //evil.com`.
     - `add_quarantine_exception()` passes an unvalidated `redir` query parameter directly to `redirect()`, allowing arbitrary `https://` targets.
     - `utils::redirect()` also interpolates `path` unescaped into the HTML fallback body (`<a href="{path}">{path}</a>`).
   - **Fix:** Validate redirect targets in one shared helper inside `utils::redirect()`: require a single leading `/` (reject `//`, `/\`, and control characters) and HTML-escape the fallback body.

6. **`-4` / `IPV4_ONLY` silently overrides `--address` and exposes localhost bindings**
   - **Where:** `src/main.rs:178-187`
   - **What happens:** `IPV4_ONLY` and `IPV6_ONLY` use `std::env::var(...).is_ok()`, so `IPV4_ONLY=false` or `IPV4_ONLY=0` still activates the flag. When active, `listener` is hardcoded to `0.0.0.0:{port}` (or `[::]:{port}`), silently discarding `--address 127.0.0.1` and binding all interfaces. Setting both flags silently prefers IPv4.
   - **Fix:** Parse the env vars as booleans, error on conflicting flags, and respect an explicit `--address` (or reject incompatible flag combinations).

7. **SFW-only mode is bypassed in listings and RSS**
   - **Where:** `src/utils.rs:1349-1381`, `src/settings.rs`, `templates/subreddit.html:62`, `templates/user.html:49`, `templates/search.html:83`, `templates/duplicates.html:54`, `src/subreddit.rs:593-617`, `src/user.rs:136-155`
   - **What happens:** Listing templates hide NSFW posts only when `prefs.show_nsfw != "on"`, and `GET /settings/update?show_nsfw=on` / `POST /settings` allow setting that cookie even when `REDLIB_SFW_ONLY=on`. Viewing `/r/all`, `/r/popular`, multireddits, or non-NSFW user profiles then exposes NSFW posts. Additionally, if subreddit/user metadata fetch fails (`subreddit.rs:112,116`, `user.rs:53`), `unwrap_or_default()` yields `nsfw = false` and skips the NSFW gate. RSS endpoints skip `sfw_only()` and NSFW gating altogether.
   - **Fix:** Force `show_nsfw = "off"` inside `Preferences::new()` whenever `sfw_only()` is true, drop `over_18` posts from listings and RSS in SFW-only mode, and fail closed when metadata lookup fails on an SFW-only instance.

8. **Invalid `redlib.toml` is silently ignored**
   - **Where:** `src/config.rs:118-123`
   - **What happens:** File read and TOML parse errors are discarded with `.ok()`, silently falling back to defaults. A single typo in `redlib.toml` quietly disables `REDLIB_SFW_ONLY` or `REDLIB_ROBOTS_DISABLE_INDEXING` with no warning.
   - **Fix:** Log parse/read errors loudly, and fail startup if a config file exists but cannot be parsed.

9. **Unbounded redirect recursion in `client::request`**
   - **Where:** `src/client.rs:300-316`
   - **What happens:** When `redirect = true` (used on the `json()` path), `request()` recurses on every 3xx response with no hop limit. If `Location` is absent, it recurses with an empty path (`unwrap_or_default()`), looping indefinitely.
   - **Fix:** Add a hop counter (e.g. max 5) and return an error when `Location` is missing.

10. **Uncaught `.expect()` panics when GitHub is unreachable**
    - **Where:** `src/main.rs:432-454`
    - **What happens:** `fetch_commit_info()` and `fetch_instances()` call `.expect("Failed to request GitHub")` and `.expect("Failed to read body")` using `CLIENT` (which has no timeout). Because `server.rs` does not catch panics, a GitHub outage resets connections, caches nothing, and causes every error page to retry and panic again.
    - **Fix:** Return `Result<String, _>`, respond with `502 Bad Gateway`, and briefly cache failures.

11. **Container workflow publishes stale binaries and runs even after build failure**
    - **Where:** `.github/workflows/build-artifacts.yaml:47`, `.github/workflows/main-docker.yml:3-12`, `Dockerfile:7`
    - **What happens:** `build-artifacts.yaml` uploads binaries to a `draft: true` GitHub release, while `Dockerfile` fetches `releases/latest/download/...` (which ignores drafts). `main-docker.yml` triggers on `workflow_run: [completed]` without checking `conclusion == 'success'`. Every push to `main`—even failed builds—can publish `sha-<commit>` and `latest` images containing an older release binary.
    - **Fix:** Gate `main-docker.yml` on `github.event.workflow_run.conclusion == 'success'`, and build the container from source or from the workflow's own artifact rather than `releases/latest`.

12. **Root `Dockerfile` fails under `compose.dev.yaml` and `heroku.yml`; `Dockerfile.ubuntu` healthcheck always fails**
    - **Where:** `Dockerfile:1,3,7`, `compose.dev.yaml:1,6`, `heroku.yml:1-3`, `Dockerfile.ubuntu:30,47`
    - **What happens:**
      - `Dockerfile` defines `ARG TARGET` with no default and pipes `curl -L` (no `-f`) into `tar`. `compose.dev.yaml` and `heroku.yml` build `.` without `TARGET`, requesting `redlib-.tar.gz` and feeding a 404 page into `tar`. `Dockerfile` also uses EOL `alpine:3.19` (while `Dockerfile.alpine` uses `3.23`).
      - `Dockerfile.ubuntu` runs `wget` in `HEALTHCHECK`, but `ubuntu:noble` does not include `wget`, leaving the container permanently `unhealthy`.
    - **Fix:** Point `compose.dev.yaml` and `heroku.yml` at a from-source Dockerfile, unify on a supported Alpine base, use `curl -fsSL`, and add a built-in `redlib --healthcheck` flag (or install `wget`).

13. **Server errors exit with status `0`; invalid `--address` panics**
    - **Where:** `src/main.rs:417-419`, `src/server.rs:386`
    - **What happens:** `if let Err(e) = server.await { eprintln!(...); }` lets `main()` return cleanly with exit code `0`, defeating Docker `restart: on-failure` and systemd `Restart=on-failure`.
    - **Fix:** Exit with a non-zero code on bind/server errors and return a clean startup error for unparsable addresses.

14. **Compression errors after reading the response body return an empty `200 OK`**
    - **Where:** `src/server.rs:368`, `src/server.rs:419`, `src/server.rs:657-680`
    - **What happens:** `compress_response()` consumes the response body with `body::to_bytes(res.body_mut())`. If `compress_body()` subsequently returns `Err`, callers discard the error (`let _ = compress_response(...)`), sending a `200` response with an empty body.
    - **Fix:** Preserve `body_bytes` and restore `*res.body_mut() = Body::from(body_bytes)` on compression failure.

15. **HLS fatal error handler retries in an infinite loop with no backoff**
    - **Where:** `static/playHLSVideo.js:70-82`
    - **What happens:** Fatal `NETWORK_ERROR` and `MEDIA_ERROR` events call `hls.startLoad()` / `hls.recoverMediaError()` unconditionally. A `403`/`429` or expired `/hls/` stream triggers a tight loop of requests against both the Redlib instance and Reddit without telling the user playback failed.
    - **Fix:** Cap retries (e.g. 3 attempts with exponential backoff), then call `hls.destroy()`, surface an error state, and fall back to the MP4 stream.

16. **RSS gating, escaping, and feed validity bugs**
    - **Where:** `src/subreddit.rs:593,608,632`, `src/user.rs:136,155-169`, `src/utils.rs:1359`
    - **What happens:**
      - Both RSS handlers check `get_setting("REDLIB_ENABLE_RSS").is_none()` instead of `utils::enable_rss()`, so `REDLIB_ENABLE_RSS=off` still enables RSS.
      - `subreddit::rss` omits `raw_json=1` (causing double-escaped titles like `&amp;amp;`) and calls `.unwrap()` on `decode_html(&post.body)`.
      - `user::rss` fetches with `raw_json=1` yet still runs `decode_html`, turning escaped user text (`&lt;img ...&gt;`) into raw markup inside `<content:encoded>`, and omits the required RSS 2.0 channel `<link>`.
    - **Fix:** Gate both handlers with `utils::enable_rss()`, use `raw_json=1` consistently without `decode_html`, handle escaping safely, and set the channel `<link>` in `user::rss`.

17. **Search queries and pagination break on `&`, `#`, `+`, and `%`**
    - **Where:** `src/search.rs:114,140,170`, `templates/search.html:4,47,63,117,123`
    - **What happens:** `search_subreddits()` only replaces spaces with `+`, so `AT&T` splits at `&` and `C++` becomes `C`. Templates embed decoded `params.q` unencoded inside `href="?q=..."` (breaking pagination and back links), while the manual `.replace('"', "&quot;")` double-escapes quotes inside auto-escaped contexts like `<title>`.
    - **Fix:** Percent-encode `q` for upstream URLs and template `href`s; remove the manual `&quot;` replacement and let Askama handle HTML escaping.

18. **Comment search matches raw HTML markup instead of comment text**
    - **Where:** `src/post.rs:154`
    - **What happens:** `query_comments()` matches against `c.body` after rendering to HTML, so queries like `href`, `class`, `md`, or `amp` match nearly every comment, and removed comments match their Pushshift URL.
    - **Fix:** Match against the raw text field (`data["body"]`) or strip HTML tags before matching.

19. **`rewrite_emotes` mispairs emote IDs, URLs, and dimensions**
    - **Where:** `src/utils.rs:1161-1215`
    - **What happens:** `id_vec` (`$[*].id`) and `link_vec` (`$[*].s.u`) are collected into separate parallel vectors. Metadata entries without `s.u` (such as Giphy GIFs) shift indices out of alignment, pairing the wrong emote ID with the wrong image URL or leaving `:emote:` codes unreplaced. Line 1215 also uses `.first().unwrap()` height for every emote.
    - **Fix:** Iterate `media_metadata.as_object()` entries directly and read `id`, `s.u`, and `s.y` from each entry.

20. **`/r/:sub/w/*page` wiki shortcut drops the page path**
    - **Where:** `src/main.rs:339-340`
    - **What happens:** The route defines `*page`, but the handler reads `r.param("wiki")`, always redirecting to `/r/<sub>/wiki/`.
    - **Fix:** Read `r.param("page")`.

21. **Broken keyboard accessibility: clipped/removed focus rings, unlabelled controls, and hover-only spoilers/blur**
    - **Where:** `static/style.css:834-836,856-865,1156-1162,1801`, `templates/base.html:31,41-46`, `templates/settings.html:56-63,102-110,142-146`, `templates/utils.html:25`, `static/playHLSVideo.js:89-90`
    - **What happens:**
      - Mobile `#search:focus { outline: 0 }` removes focus completely; `overflow-y: hidden` on `#sort_options`, `#listing_options`, and pagination links clips `:focus-visible` rings, and the active tab's outline matches its `--accent` fill.
      - Spoilers (`.md-spoiler-text`) and blurred NSFW/spoiler media (`.post_blurred`) reveal only on `:hover`, making them inaccessible to keyboard users and awkward on touch screens.
      - `<div id="video_quality">` inside `<head>` (`base.html:31`) is invalid HTML and duplicates `id="video_quality"` on `<select>` in `settings.html:57`, breaking `<label for="video_quality">`.
      - `post_sort` select and three settings checkboxes (`fixed_navbar`, `hide_sidebar_and_summary`, `disable_visit_reddit_confirmation`) have no `id`, leaving their `<label for="...">` disconnected and shrinking click/tap targets to 13 px.
      - Search submit buttons, the mobile Reddit nav icon (when its `<span>` hides at <= 800 px), and the JS-injected HLS quality `<select>` lack accessible names.
    - **Fix:** Restore visible focus styles with `outline-offset`, move `video_quality` to a `data-*` attribute on `<body>`, add missing `id`/`aria-label` attributes, and support `:focus-within` / explicit reveal controls for spoilers and blurred media.

22. **Unreadable visited post titles and low-contrast UI/accent tokens across themes**
    - **Where:** `static/style.css:95,1031,1278,1528,1531`, `static/themes/*.css`
    - **What happens:**
      - Visited post titles drop to 1.2:1–2.1:1 contrast in `icebergDark` (`#0f1117` on `#1f2233`), `tokyoNight` (`#414868` on `#1a1b26`), and `gruvboxdark`/`gruvboxlight`/`rosebox` (which omit `--visited` and inherit the OS-scheme `:root` value).
      - Meaningful metadata and links at `0.4`–`0.5` opacity (`.post_footer`, `.created`, `.edited`, `.search_subreddit_description`) drop to 2.0:1–3.9:1 across most themes, including default `light`.
      - Button/selected-tab text (`--foreground` on `--accent`) falls below WCAG AA (4.5:1) in default `dark` (3.6:1), `black` (3.2:1), `libredditLight` (3.2:1), `icebergDark` (3.1:1), and `tokyoNight` (2.4:1; its accent on post background is also 2.8:1).
      - Links in `.md` (post bodies and sidebars) rely on colour alone (`text-decoration: none` globally; only `.comment_body a` is underlined).
    - **Fix:** Define explicit, accessible `--visited` and muted-text tokens in every theme, raise accent/button contrast to >= 4.5:1, and underline links inside `.md`.

23. **Layout breakage: settings row overlap, clipped exit popup, and fixed navbar covering mobile pages**
    - **Where:** `static/style.css:164-167,236-340,1678-1684,1982+`
    - **What happens:**
      - `.prefs-group { height: 35px }` overflows and collides with adjacent rows when longer labels wrap to 2–3 lines.
      - `.popup` (`height: 100vh; overflow: clip`) with non-scrolling `.popup-inner` clips the "Yes / No" buttons on short/landscape mobile viewports (~360 px tall).
      - With `fixed_navbar` enabled at <= 800 px, the two-row navbar (~95–100 px tall) overlaps the top of pages without manual margin patches (`#error`, `#nsfw_landing`, `#wiki`, `#duplicates`, `/info`), because `body.fixed_navbar` only adds `padding-top: 45px`.
    - **Fix:** Replace fixed `height: 35px` with `min-height: 35px; height: auto`, allow `.popup-inner` to scroll (`overflow-y: auto; max-height: 90dvh`), and drive `body.fixed_navbar` offset from a CSS custom property updated at the <= 800 px breakpoint.

24. **Typography bugs: pixel-locked type, missing body `line-height`, and FOIT on 325 KB font**
    - **Where:** `static/style.css:33-38,153-158`
    - **What happens:**
      - All font sizes are set in `px` (`body { font-size: 15px }`), ignoring browser default font-size preferences (WCAG 1.4.4).
      - No `line-height` is set on `body`, `.md`, `.comment_body`, or `.post_body`, so long comment threads render at Inter's tight `normal` (~1.21).
      - `@font-face` for `static/Inter.var.woff2` (325 KB) omits `font-display`, causing up to 3 seconds of invisible text (FOIT) on cold loads, with a fallback of only generic `sans-serif`.
    - **Fix:** Use `rem` sizes, set `line-height: 1.5`–`1.6` for prose/comments, and add `font-display: swap` with a clean system-UI fallback stack.

25. **OpenSearch descriptor is hardcoded to `https://localhost:8080`**
    - **Where:** `static/opensearch.xml:6-7,10`, `src/main.rs:248`
    - **What happens:** Served statically with a 14-day cache, so browser "Add search engine" points to localhost on every deployed instance.
    - **Fix:** Render `opensearch.xml` dynamically using `REDLIB_FULL_URL` or the request `Host` header.

26. **Update checker always fails with `TypeError` on outdated instances**
    - **Where:** `static/check_update.js:24,27`, `templates/error.html:9,11`
    - **What happens:** `document.getElementById('error-446').remove()` throws because no `#error-446` element exists in any template. The `catch` block overwrites the status with `"Error checking update status: TypeError..."`, so users never see how many commits behind the instance is. `error.html` also duplicates `id="update-status"`.
    - **Fix:** Guard with `?.remove()` (or add the element) and deduplicate `#update-status`.

27. **`/info.txt` prints mismatched configuration values**
    - **Where:** `src/instance_info.rs:169-173`, `src/instance_info.rs:197-201`
    - **What happens:** The format labels list `SFW only`, `Pushshift frontend`, `RSS enabled`, `Full URL`, `Remove default feeds`, but the arguments pass `sfw_only`, `enable_rss`, `full_url`, `default_remove_default_feeds`, `pushshift`, shifting four fields out of order.
    - **Fix:** Align the format arguments with the labels.

28. **Configuration drift: broken Heroku env vars and unwired `REDLIB_DEFAULT_AUTOPLAY_VIDEOS`**
    - **Where:** `app.json:41,44`, `.env.example:38`, `README.md:438`, `src/config.rs:163-191`
    - **What happens:** `app.json` uses `REDLIB_USE_HLS` and `REDLIB_HIDE_HLS_NOTIFICATION` instead of `REDLIB_DEFAULT_USE_HLS` and `REDLIB_DEFAULT_HIDE_HLS_NOTIFICATION`. Meanwhile, `.env.example` and `README.md` document `REDLIB_DEFAULT_AUTOPLAY_VIDEOS`, and `Preferences` reads the `autoplay_videos` cookie, but `Config` has no `default_autoplay_videos` field—so the server-wide default is silently ignored.
    - **Fix:** Rename the keys in `app.json` and wire `default_autoplay_videos` into `Config` (or remove it from `.env.example` and `README.md`).

---

## High-Value Improvements

29. **Set outbound HTTP timeouts and a graceful shutdown deadline** *(Reliability)*
    - **Where:** `src/client.rs:90-95`, `src/server.rs:389-403`
    - **Impact:** `CLIENT` sets no `.connect_timeout()`, `.read_timeout()`, or `.timeout()`. A stalled upstream connection hangs request tasks indefinitely and blocks graceful shutdown until Docker sends `SIGKILL`.
    - **Direction:** Configure connect (~5 s) and request/read (~15–30 s) timeouts on `CLIENT` (separate streaming timeout for media if needed) and bound graceful shutdown with a timeout.

30. **Lower Brotli quality from 11 and move compression off Tokio worker threads** *(Performance)*
    - **Where:** `src/server.rs:695-738`
    - **Impact:** `BrotliEncoderParams::default()` runs at quality `11` synchronously on the async executor for every HTML/JSON response >= 1452 bytes sent to browsers advertising `br`, stalling workers for tens to hundreds of milliseconds per response.
    - **Direction:** Use quality `4`–`5` (`lgwin` `20`) and run compression in `tokio::task::spawn_blocking` (or stream it). Also revisit `compress_body`'s cache, which hashes and clones entire `Vec<u8>` bodies into memory for dynamic pages that rarely hit.

31. **Avoid constructing `Preferences::new(req)` and re-parsing cookies for every comment** *(Performance)*
    - **Where:** `src/post.rs:252`, `src/server.rs:226-241`, `templates/comment.html:9,29`
    - **Impact:** Rendering a thread with 1,000 comments allocates the theme list 1,000 times and parses the entire `Cookie` header ~25,000+ times, even though `comment.html` only reads `hide_score` and `hide_awards`.
    - **Direction:** Parse request cookies once per request and pass `&Preferences` (or the two booleans) down the comment tree.

32. **Catch handler panics at the server boundary** *(Reliability)*
    - **Where:** `src/server.rs:345-380`
    - **Impact:** Any unexpected panic in a handler drops the TCP connection without a response instead of returning `500 Internal Server Error`.
    - **Direction:** Wrap route dispatch with `futures_lite::FutureExt::catch_unwind` (already a dependency), log the panic, and return a clean 500 error page.

33. **Validate short-link IDs before calling Reddit** *(Reliability)*
    - **Where:** `src/main.rs:395`
    - **Impact:** Any 5–7 character path (`/login`, `/admin`, `/xmlrpc`) triggers `canonical_path()`, firing up to 6 upstream HEAD requests to Reddit and thrashing the 1024-entry cache—increasing IP rate-limit/ban risk from routine internet scanners.
    - **Direction:** Require ASCII base-36 (`[a-z0-9]`) before calling `canonical_path()`, and similarly validate share-link IDs.

34. **Add request-level observability and a meaningful `/healthz` + `--healthcheck`** *(Reliability & Debugging)*
    - **Where:** `src/server.rs`, `src/oauth.rs`, `Dockerfile*`, `compose.yaml:19-20`
    - **Impact:** There is no request logging, error-rate visibility, or upstream 429/rate-limit telemetry, and the current `/settings` healthcheck reports healthy even when OAuth is dead or Reddit is blocking the instance.
    - **Direction:** Emit lightweight structured access/error logs (method, route template, status, duration—no client IPs), expose OAuth/upstream health via `/healthz`, and add a `--healthcheck` CLI mode so containers don't depend on `wget`.

35. **Cache static JS assets and stop loading 412 KB `hls.min.js` on pages without HLS video** *(Performance)*
    - **Where:** `src/main.rs:250-259`, `templates/subreddit.html:68`, `templates/search.html:110`, `templates/user.html:85`
    - **Impact:** All JS files—including 412 KB `hls.min.js`—are served with `cache = false`, and listing pages load `hls.min.js` synchronously even when no post has an HLS stream.
    - **Direction:** Enable long-lived caching with `?v={{ env!("CARGO_PKG_VERSION") }}` (matching `style.css`), emit `hls.min.js` only when at least one post has `alt_url`, and add `defer`.

36. **Separate live-Reddit tests from offline unit tests and stop masking failures with 10 retries** *(Dev & Debugging)*
    - **Where:** `.github/workflows/pull-request.yml:5`, `src/client.rs:659-703`, `src/oauth.rs:476-520`, `src/subreddit.rs:712-718`, `src/user.rs:190`
    - **Impact:** `NEXTEST_RETRIES: 10` hides real flakiness/regressions while Reddit IP throttling still breaks PRs. Meanwhile, `post.rs`, `search.rs`, `duplicates.rs`, `settings.rs`, `instance_info.rs`, routing, OAuth refresh, and CLI bind logic have virtually no offline test coverage—nearly every confirmed bug above would be caught by fast fixture tests.
    - **Direction:** Mark live-network tests `#[ignore]` (run on a non-blocking schedule), add JSON-fixture unit tests for parsing, escaping, routing, RSS, OAuth rollover, and settings, and drop the 10x retry crutch.

37. **Fix CI coverage, deduplicate release builds, and pin supply-chain dependencies** *(Dev & Reliability)*
    - **Where:** `.github/workflows/*.yaml`, `Cargo.toml:62-63`, `Dockerfile:7`
    - **Impact:**
      - Musl cross-builds run twice per push (`build-artifacts.yaml` and `main-rust.yml`) and never on PRs, so broken container/cross builds are only discovered post-merge.
      - `pull-request.yml` runs without `--locked`, without `Swatinem/rust-cache`, without `clippy --all-targets`, and uses outdated `actions/checkout@v3`.
      - Core HTTP/TLS emulation relies on caret-versioned RC crates (`wreq = "6.0.0-rc.28"`, `wreq-util = "3.0.0-rc.10"`), alongside legacy/deprecated crates (`hyper 0.14`, `bincode 1.3`, `serde_yaml 0.9`).
    - **Direction:** Pin RC crates with `=`, enforce `--locked` in CI, add `rust-cache` and `clippy --all-targets`, add a PR container/musl smoke build, consolidate the duplicate release workflows, verify release tarball SHA-256 checksums in `Dockerfile`, and add `cargo-audit` / `cargo-deny`.

38. **Replace misleading Reddit error messages** *(UX & Reliability)*
    - **Where:** `src/client.rs:410-480`
    - **Impact:** Upstream `403` and `429` HTML responses surface as `"Failed to parse page JSON data"`, and empty non-429 bodies claim `"rate limit exceeded"` while triggering an unnecessary token refresh.
    - **Direction:** Map HTTP `429` and `403` explicitly to clear rate-limit/blocked messages and only trigger token refresh on `401` / genuine `429`.

39. **Stop swallowing template rendering errors** *(Reliability)*
    - **Where:** `src/utils.rs:1296-1302` (and `error()`, `info()`, `nsfw_landing()`, `quarantine()`)
    - **Impact:** `t.render().unwrap_or_default()` silently serves an empty `200 OK` page with no log entry if Askama fails.
    - **Direction:** Log the render error and return a `500` plain-text fallback.

40. **Fix comment thread hierarchy, collapse ergonomics, and deep-nesting crush** *(UX & Accessibility)*
    - **Where:** `static/style.css:1537`, `templates/comment.html:44`
    - **Impact:** Thread guide lines (`.line`) have ~1.1–1.2:1 contrast and are not clickable to collapse from partway down a long comment; replies use `<blockquote class="replies">` (announced as quotations by screen readers); and uncapped indentation crushes text to ~20 characters wide by depth 8 on mobile.
    - **Direction:** Use `--panel-border` contrast for `.line`, allow clicking the thread line to collapse, replace `<blockquote>` with `<div role="group">`, and cap indentation past a sensible depth on narrow viewports.

41. **Improve mobile feed layout, touch targets, post-title scale, and wide-mode reading measure** *(UX & Typography)*
    - **Where:** `static/style.css:356,1022,1315,1764,1936`
    - **Impact:**
      - At <= 800 px, `main { flex-direction: column-reverse }` puts the expanded subreddit `<details>` panel above the post list on every page load.
      - Nav icons and `.post_footer` links lack touch-target padding (< 44 px).
      - `.post_title` (16 px) is smaller than markdown `h1`/`h2` inside post bodies (22 px / 20 px), flattening visual hierarchy.
      - `.wide #column_one { max-width: 100% }` lets prose stretch past 200 characters per line on desktop monitors.
    - **Direction:** Collapse the subreddit sidebar by default on mobile (or place it after posts), increase touch targets, scale single-post `h1` above body headings (~22 px/600), and cap prose measure (`.md`, `.comment_body`, `.post_body`) at ~70 ch.

42. **Improve error, empty, gallery, and clipboard/settings feedback states** *(UX)*
    - **Where:** `templates/error.html`, `templates/subreddit.html`, `templates/search.html`, `templates/utils.html:139-152`, `static/copy.js`, `src/settings.rs:85,280-298`
    - **Impact:**
      - Error pages lack a retry action; empty feeds show bare `<center>No posts were found.</center>` with no recovery cue; settings save and copy give no visual confirmation (`copy.js` fails silently over plain HTTP / `.onion` where `navigator.clipboard` is undefined); restore errors show raw internal messages on a separate error page.
      - Galleries omit "1 of N" counts, use generic `"Gallery image"` alt text, emit an empty `<p>` when captions are absent, and leave the outbound `<p>` unclosed (`templates/utils.html:147`).
    - **Direction:** Add reload/clear-filter recovery links, inline confirmation/error feedback for settings and copy (with `execCommand` fallback), gallery counters + post-title alt text, and close the unclosed `<p>`.

43. **Add absolute URLs to Open Graph / Twitter metadata** *(UX)*
    - **Where:** `templates/post.html:30-35`
    - **Impact:** `og:image`, `twitter:image`, and `og:video` emit relative paths (`/thumb/...`, `/img/...`), so chat apps and link-preview crawlers cannot render thumbnails.
    - **Direction:** Prefix with `REDLIB_FULL_URL` (or the request scheme/host) when available.

44. **Add a skip link and proper page landmarks / heading levels** *(Accessibility & UX)*
    - **Where:** `templates/base.html`, feed templates
    - **Impact:** Keyboard users must tab through ~6 navbar controls on every page load before reaching content, and feed pages have no `<h1>`.
    - **Direction:** Add a visually hidden (focus-revealed) "Skip to content" link targeting `<main>` and ensure each view has one meaningful `<h1>`.

---

## Medium-Value Improvements

45. **Add HTTP/1 header-read timeout and connection hardening** *(Reliability)* — `src/server.rs:389`: hyper 0.14 leaves `http1_header_read_timeout` unset, leaving public instances open to slowloris connection exhaustion.
46. **Add `Vary: Accept-Encoding` when compressing responses** *(Protocol)* — `src/server.rs:672-674`: prevents shared proxies/CDNs from serving Brotli/Gzip bodies to clients that do not support them.
47. **Eliminate duplicate concurrent fetches and serial lookups** *(Performance)* — `src/client.rs:406`, `src/subreddit.rs:289`, `src/post.rs:116,142`: add single-flight coalescing on `client::json` so thundering-herd requests don't burn Reddit rate limits; avoid fetching a full hot listing just to resolve display names; iterate comment children by reference instead of cloning.
48. **Fix unencoded flair/redirect links and invalid/duplicate HTML markup** *(UX & Correctness)* — `templates/utils.html:93,135,245,282`, `templates/duplicates.html:78`, `templates/post.html:54-65`, `templates/wall.html:8`, `templates/nsfwlanding.html:24`: URL-encode flair search links and redirect query params (which currently break on `&`, `#`, `+`, `%`), quote `href="{{ post.media.url }}"`, and fix duplicate `#search`/`#sort` IDs, `<legend>` outside `<fieldset>`, and `<button>` inside `<a>`.
49. **Make `Preferences` export versioning actually work** *(Reliability & Dev)* — `src/utils.rs:620-670`, `src/settings.rs:295`: `#[revisioned(revision = 1)]` is declared on `Preferences`, yet export/import uses plain `bincode::serialize`/`deserialize`, so any added field silently breaks all existing exported preference strings. Use the `revision` crate's serialiser or an explicit version header.
50. **Filter invalid/missing gallery items instead of emitting empty `<img>`/`<a>` tags** *(UX)* — `src/utils.rs:296-320`, `templates/utils.html:139-152`: skip items whose metadata has no `s` or whose `status != "valid"`.
51. **Set `color-scheme` in all theme files and respect `prefers-reduced-motion` / `forced-colors`** *(UX & Accessibility)* — `static/style.css`, `static/themes/*.css`: 15 of 18 themes omit `color-scheme`, leaving light scrollbars and native selects on dark themes when the OS is in light mode; video autoplay and transitions ignore `prefers-reduced-motion`.
52. **Fix `highlighted.js` on image and video posts** *(UX)* — `templates/post.html:40`: the `{% if single_thread %}` script include is nested inside an `{% else %}` branch, so scroll-to-comment fails on image/video posts.
53. **Clean up diagnostics and broken `ResponseExt::cookies`** *(Debugging & Security)* — `src/oauth.rs:285`, `src/server.rs:243-252`: demote bearer-token prefix and device UUID logging from `info!` to `trace!`; `ResponseExt::cookies()` reads the `Cookie` header instead of `Set-Cookie` (fix or remove dead code).
54. **Improve monospace font stack, score screen-reader labels, and theme names** *(Typography & UX)* — `static/style.css:1834`, `templates/utils.html`, `src/utils.rs`: replace `monospace, sans-serif` with `ui-monospace, "SF Mono", Menlo, Consolas, monospace` plus inline-code padding; use `.sr-only` instead of `display: none` for upvote labels; humanise theme display names (`LibredditBlack` -> `Libreddit Black`) and add a "Reset to defaults" button on `/settings`.
55. **Tighten container build context and dev ergonomics** *(Dev & Ops)* — `.dockerignore`, `.devcontainer/devcontainer.json`, `flake.nix`: exclude `.git` from `.dockerignore` to stop invalidating Docker cache on every commit; stop running `cargo build` on every devcontainer start; pin the Rust toolchain in `flake.nix` to `rust-version` (`1.81`) and re-enable checks; add a `justfile` for standard `fmt`/`clippy`/`test` commands.
