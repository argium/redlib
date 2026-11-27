# Redlib Audit Remediation, Slice 1 — Task List

- **Status:** Ready
- **Author:** Argi
- **Created:** 2026-11-28
- **Language:** Rust
- **Source:** [plan.md](./plan.md), Slice 1 (S1): OAuth Lifecycle Safety, Upstream Client Resilience & Offline Test Harness

## Summary

Redlib keeps one shared Reddit OAuth token for every visitor. Today a cancelled or failed token refresh can stop the whole instance from serving pages, or kill the process outright, and Reddit or GitHub stalls can hang requests forever. These tasks make token refresh safe to cancel and fail, bound every upstream call, and give each fix a regression test that runs without the network.

## Starting Context

Existing code the tasks build on (verified — do not re-explore):

- `src/oauth.rs:73` — `pub(crate) async fn new() -> Self` retries forever-ish and calls `std::process::exit(1)` at `src/oauth.rs:111` after 10 failures. `OAUTH_CLIENT` (`src/client.rs:35`) calls it at startup via `block_on`.
- `src/oauth.rs:118` — `new_with_timeout_with_backend(backend) -> Result<Result<Self, AuthError>, Elapsed>` performs one authentication attempt.
- `src/oauth.rs:160` — `token_daemon()` sleeps `expires_in - 120` seconds then calls `force_refresh_token()`.
- `src/oauth.rs:182` — `force_refresh_token()` sets `OAUTH_IS_ROLLING_OVER` with `compare_exchange` and clears it only on the last line.
- `src/oauth.rs:254-263` and `src/oauth.rs:367-376` — duplicated `x-reddit-loid` / `x-reddit-session` copying with `to_str().unwrap()`.
- `src/oauth.rs:285` — `&token[..32]` panics on a token shorter than 32 bytes. `src/oauth.rs:400` already uses `32.min(token.len())`.
- `src/oauth.rs:324` and `src/oauth.rs:453` — `info!` lines that print the generated device ID and UUID.
- `src/client.rs:76` — `build_client()` builds `CLIENT` with no timeouts.
- `src/client.rs:243`, `:248`, `:261` — `reddit_get`, `reddit_short_head`, and `request(method, path, redirect, quarantine, base_path, host)`. `request` recurses on every 3xx at `:308` and uses `unwrap_or_default()` when `Location` is missing.
- `src/client.rs:429-516` — `json()` reads rate-limit headers, treats an empty body as a rate limit (`:454`), awaits `force_refresh_token()` inline on `Unauthorized` (`:485`), and maps unparsable bodies at `:513`.
- `src/main.rs:422-454` — `proxy_commit_info`/`proxy_instances` and their `#[cached(time = 600)]` fetchers that `.expect()` on network errors.
- Live-network tests: `src/client.rs` (`test_rate_limit_check`, `test_default_subscriptions`, `test_localization_popular`, `test_obfuscated_share_link`, `test_private_sub`, `test_banned_sub`, `test_gated_sub`), `src/oauth.rs` (`test_mobile_spoof_backend`, `test_oauth_client`, `test_oauth_client_refresh`, `test_oauth_token_exists`, `test_oauth_headers_len`), `src/subreddit.rs:712-723`, `src/user.rs:190`, `src/utils.rs:1561-1583`.

Conventions and gotchas:

- Tabs for indentation (`rustfmt.toml`); `cargo fmt --check` and `cargo clippy --locked --all-targets` are clean on `main`.
- Windows builds need `cmake` and `nasm` on `PATH` for `btls-sys` (see [plan.md Prerequisites](./plan.md#prerequisites)).
- Any test that touches `OAUTH_CLIENT` or `client::json` hits live Reddit. Regression tests in this slice must not.
- `wreq::header` re-exports `http::header` (`HeaderMap`, `HeaderValue`).

## Tasks

### Task 1: Default test run is offline

#### Problem

About 18 existing tests call live Reddit (via `OAUTH_CLIENT`, `client::json`, or `Post::fetch`). CI retries them up to 10 times, and they fail whenever Reddit throttles the runner, so a red run says nothing about the code.

#### Solution

- Mark each live-network test `#[ignore = "requires live Reddit"]`. Keep the tests; they still run with `cargo test -- --ignored`.
- Leave the existing `#[ignore = "todolol"]` on `test_generic_web_backend` as is.

#### Notes

- **Location:** `src/client.rs`, `src/oauth.rs`, `src/subreddit.rs`, `src/user.rs`, `src/utils.rs`

- **Verify:** `cargo test --locked -- --list --ignored` lists all 18 tests, and `cargo test --locked` passes with the network disabled (or, if that is impractical, passes and its output shows `18 ignored` plus the existing one).

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Test (red)** — none. This task changes test selection only; Task 9 runs the whole offline suite as its regression evidence.

**Implementation guidance (green)**:

- Add `#[ignore = "requires live Reddit"]` directly under the `#[tokio::test(...)]` (or `#[test]`) attribute of each test listed in Starting Context.
- For `client::tests::test_default_subscriptions`, put the `#[ignore]` between `#[test]` and `#[sealed_test(...)]`; if `sealed_test` rejects that order, put it after `#[sealed_test(...)]` and confirm with `cargo test --locked -- --list --ignored`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M1.5

</details>

- [x] Task 1 complete

---

### Task 2: A cancelled or panicking token refresh releases the rollover flag

#### Problem

`OAUTH_IS_ROLLING_OVER` stops two refreshes from running at once. `force_refresh_token()` clears it only on its last line, and `client::json()` awaits the refresh inside the visitor's request. If the visitor closes the tab (hyper drops the request future) or the refresh panics, the flag stays set and every later refresh is skipped until restart. Two panics are reachable today: `&token[..32]` on a short token and `to_str().unwrap()` on a non-ASCII header.

#### Solution

- Add a private `RolloverGuard` that sets the flag on acquire and clears it on drop.
- `force_refresh_token()` holds the guard for the whole refresh.
- `client::json()` spawns the `Unauthorized` refresh with `tokio::spawn` instead of awaiting it.
- One helper copies the session headers, skipping values that are not visible ASCII; a `token_preview` helper replaces both token slices.

#### Notes

- **Location:** `src/oauth.rs`, `src/client.rs`

- **Verify:** `cargo test --locked --lib oauth::tests` passes, including the four new tests.

- Boundary: unit tests on a local `AtomicBool` and header map; `force_refresh_token()` itself still needs live Reddit and is covered by Task 9's review.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — add to `src/oauth.rs` above `force_refresh_token` (and `use std::sync::atomic::AtomicBool;`):

```rust
/// Holds the token-rollover flag for one refresh attempt.
struct RolloverGuard(&'static AtomicBool);

impl RolloverGuard {
	/// Returns `None` when another refresh already holds the flag.
	fn try_acquire(flag: &'static AtomicBool) -> Option<Self> {
		flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Self(flag))
	}
}

fn remember_session_headers(_response_headers: &wreq::header::HeaderMap, _remembered: &mut HashMap<String, String>) {}

fn token_preview(token: &str) -> &str {
	token
}
```

**Test (red)** — add to `mod tests` in `src/oauth.rs`:

```rust
	#[test]
	fn rollover_guard_allows_one_holder_and_releases_on_drop() {
		static FLAG: AtomicBool = AtomicBool::new(false);
		let guard = RolloverGuard::try_acquire(&FLAG).expect("first refresh acquires the flag");
		assert!(RolloverGuard::try_acquire(&FLAG).is_none(), "a second refresh must be skipped while one is running");
		drop(guard);
		assert!(!FLAG.load(Ordering::SeqCst));
		assert!(RolloverGuard::try_acquire(&FLAG).is_some(), "a later refresh can run again");
	}

	#[test]
	fn rollover_guard_releases_when_refresh_panics() {
		static FLAG: AtomicBool = AtomicBool::new(false);
		let outcome = std::panic::catch_unwind(|| {
			let _guard = RolloverGuard::try_acquire(&FLAG).unwrap();
			panic!("authentication panicked");
		});
		assert!(outcome.is_err());
		assert!(!FLAG.load(Ordering::SeqCst));
	}

	#[tokio::test]
	async fn rollover_guard_releases_when_refresh_future_is_cancelled() {
		static FLAG: AtomicBool = AtomicBool::new(false);
		let refresh = async {
			let _guard = RolloverGuard::try_acquire(&FLAG).unwrap();
			std::future::pending::<()>().await;
		};
		// A visitor disconnecting drops the request future part-way through the refresh.
		assert!(tokio::time::timeout(Duration::from_millis(10), refresh).await.is_err());
		assert!(!FLAG.load(Ordering::SeqCst));
	}

	#[test]
	fn remember_session_headers_skips_non_ascii_values() {
		let mut response = wreq::header::HeaderMap::new();
		response.insert("x-reddit-loid", wreq::header::HeaderValue::from_static("loid-123"));
		response.insert("x-reddit-session", wreq::header::HeaderValue::from_bytes(b"caf\xe9").unwrap());
		let mut remembered = HashMap::new();
		remember_session_headers(&response, &mut remembered);
		assert_eq!(remembered.get("x-reddit-loid").map(String::as_str), Some("loid-123"));
		assert!(!remembered.contains_key("x-reddit-session"));
	}

	#[test]
	fn token_preview_never_panics() {
		assert_eq!(token_preview("short"), "short");
		assert_eq!(token_preview(&"a".repeat(40)), "a".repeat(32));
		assert_eq!(token_preview(&"é".repeat(40)).chars().count(), 32);
	}
```

Run `cargo test --locked --lib oauth::tests`; the three guard tests fail because the stub has no `Drop`, the header test fails because nothing is copied, and the preview test fails because the stub returns the whole token.

**Implementation guidance (green)**:

- `impl Drop for RolloverGuard { fn drop(&mut self) { self.0.store(false, Ordering::SeqCst); } }`
- `remember_session_headers`: for `name` in `["x-reddit-loid", "x-reddit-session"]`, insert `value.to_str()` only when it is `Ok`.
- `token_preview`: `token.char_indices().nth(32).map_or(token, |(end, _)| &token[..end])`.
- Replace `src/oauth.rs:254-263` and `:367-376` with `remember_session_headers(resp.headers(), &mut self.additional_headers);`.
- Replace `&token[..32]` (`:285`) and `&token[..32.min(token.len())]` (`:400`) with `token_preview(&token)`.
- In `force_refresh_token`: `let Some(_guard) = RolloverGuard::try_acquire(&OAUTH_IS_ROLLING_OVER) else { trace!(...); return; };` and delete the final `OAUTH_IS_ROLLING_OVER.store(false, ...)`.
- `src/client.rs:485`: replace `let () = force_refresh_token().await;` with `tokio::spawn(force_refresh_token());`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 1

- **Supports:** M1.1

</details>

- [x] Task 2 complete

---

### Task 3: A failed runtime refresh keeps the current token instead of exiting

#### Problem

`Oauth::new()` tries the Android backend 5 times, the web backend 5 more, then calls `std::process::exit(1)`. Startup and runtime refreshes share it, so a two-minute Reddit outage during a scheduled refresh kills a healthy instance.

#### Solution

- Move the retry-and-fallback loop into `authenticate_with_fallback`, which returns the last error instead of exiting.
- Add `Oauth::try_new() -> Result<Self, String>`. `Oauth::new()` (startup only) calls it and still exits non-zero on failure.
- `force_refresh_token()` returns `bool`: `false` only when it attempted a refresh and it failed, keeping the current client. `token_daemon` retries after 60 seconds when it gets `false`, and uses `saturating_sub` for the sleep.

#### Notes

- **Location:** `src/oauth.rs`

- **Verify:** `cargo test --locked --lib oauth::tests::authentication` passes both new tests.

- Boundary: the retry policy is tested with a fake attempt closure; real backends are not called.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — add to `src/oauth.rs`:

```rust
async fn authenticate_with_fallback<T, F, Fut>(_attempt: F, _retry_delay: Duration) -> Result<T, String>
where
	F: FnMut(OauthBackendImpl) -> Fut,
	Fut: std::future::Future<Output = Result<T, String>>,
{
	unimplemented!("Task 3")
}
```

**Test (red)** — add to `mod tests` in `src/oauth.rs`:

```rust
	#[tokio::test]
	async fn authentication_gives_up_with_an_error_after_falling_back_to_web_auth() {
		let mut used_web_backend = Vec::new();
		let outcome: Result<(), String> = authenticate_with_fallback(
			|backend| {
				used_web_backend.push(matches!(backend, OauthBackendImpl::GenericWeb(_)));
				async { Err("reddit unavailable".to_string()) }
			},
			Duration::ZERO,
		)
		.await;
		assert_eq!(outcome, Err("reddit unavailable".to_string()));
		assert_eq!(used_web_backend, [vec![false; 5], vec![true; 5]].concat());
	}

	#[tokio::test]
	async fn authentication_returns_the_first_success() {
		let mut attempts = 0;
		let outcome = authenticate_with_fallback(
			|_| {
				attempts += 1;
				let attempt = attempts;
				async move { if attempt < 3 { Err("flaky".to_string()) } else { Ok(attempt) } }
			},
			Duration::ZERO,
		)
		.await;
		assert_eq!(outcome, Ok(3));
	}
```

Run `cargo test --locked --lib oauth::tests::authentication`; both fail because the stub panics with `not implemented: Task 3`.

**Implementation guidance (green)**:

- Add `const OAUTH_FALLBACK_AFTER_FAILURES: u32 = 5;`, `const OAUTH_MAX_FAILURES: u32 = 10;`, and `const OAUTH_RETRY_AFTER_FAILED_REFRESH: Duration = Duration::from_secs(60);`.
- `authenticate_with_fallback`: start with `OauthBackendImpl::MobileSpoof(MobileSpoofAuth::new())`; on each `Err(e)` increment `failures`, `error!` with `failures`/`OAUTH_MAX_FAILURES`, return `Err(e)` at the maximum, switch to `GenericWeb(GenericWebAuth::new())` (with the existing `warn!`) when `failures == OAUTH_FALLBACK_AFTER_FAILURES`, then `tokio::time::sleep(retry_delay).await`.
- Replace `new_with_timeout_with_backend` with `async fn authenticate_once(backend) -> Result<Self, String>` that flattens the timeout and `AuthError` cases into the existing message text (`src/oauth.rs:86-97`).
- `pub(crate) async fn try_new() -> Result<Self, String> { authenticate_with_fallback(Self::authenticate_once, OAUTH_TIMEOUT).await }`; log `info!("[✅] Successfully created OAuth client")` on success.
- `new()`: `Self::try_new().await.unwrap_or_else(|e| { error!("[⛔] Failed to create OAuth client (mobile + generic): {e}"); std::process::exit(1) })`.
- `force_refresh_token() -> bool`: return `true` when skipped (another refresh holds the guard); on `Err(e)` log `error!("[⛔] OAuth token refresh failed; keeping the current token: {e}")` and return `false`; on success swap the client, reset the rate limit, return `true`.
- `token_daemon`: `Duration::from_secs(expires_in.saturating_sub(120))`, then `while !force_refresh_token().await { tokio::time::sleep(OAUTH_RETRY_AFTER_FAILED_REFRESH).await; }`.
- `rate_limit_check` (`src/client.rs`) ignores the returned `bool`; `tokio::spawn(force_refresh_token())` call sites compile unchanged.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 2 (both edit `force_refresh_token`)

- **Supports:** M1.2

</details>

- [x] Task 3 complete

---

### Task 4: Stalled upstream connections time out

#### Problem

`CLIENT` serves every Reddit API call, media proxy request (`/img`, `/vid`, `/hls`) and GitHub fetch, but it has no timeouts. A connection that opens and never answers holds the visitor's request and a socket until the process restarts.

#### Solution

- Add `build_client_with_timeouts(connect_timeout, read_timeout)`; `build_client()` calls it with 10 s connect and 30 s read.
- The read timeout is an idle timeout: wreq resets it after every chunk, so long media streams keep working while a silent upstream fails. No total timeout is set for that reason.

#### Notes

- **Location:** `src/client.rs`

- **Verify:** `cargo test --locked --lib client::tests::client_times_out` passes in under 5 seconds.

- Boundary: loopback TCP listener that accepts and never replies; no Reddit traffic.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — split `src/client.rs:76` so the test compiles:

```rust
pub fn build_client() -> WreqClient {
	build_client_with_timeouts(CONNECT_TIMEOUT, READ_TIMEOUT)
}

fn build_client_with_timeouts(_connect_timeout: Duration, _read_timeout: Duration) -> WreqClient {
	// existing body of build_client, unchanged
}
```

with `use std::time::Duration;`, `const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);` and `const READ_TIMEOUT: Duration = Duration::from_secs(30);`.

**Test (red)** — add to `mod tests` in `src/client.rs`:

```rust
	#[tokio::test(flavor = "multi_thread")]
	async fn client_times_out_when_upstream_stops_responding() {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		// Accept the connection, then never send a response.
		let silent_upstream = tokio::spawn(async move {
			let (_socket, _) = listener.accept().await.unwrap();
			std::future::pending::<()>().await;
		});

		let client = build_client_with_timeouts(Duration::from_secs(1), Duration::from_millis(200));
		let outcome = tokio::time::timeout(Duration::from_secs(5), client.get(format!("http://{address}/")).send()).await;
		silent_upstream.abort();

		let error = outcome.expect("the client must give up before the test deadline").expect_err("a silent upstream must fail");
		assert!(error.is_timeout(), "{error}");
	}
```

Run `cargo test --locked --lib client::tests::client_times_out`; it fails with "the client must give up before the test deadline" because the stub ignores the timeouts.

**Implementation guidance (green)**:

- In `build_client_with_timeouts`, add `.connect_timeout(connect_timeout)` and `.read_timeout(read_timeout)` to the builder chain before `.build()`.
- Comment `READ_TIMEOUT` with why it is an idle timeout rather than a total one (media streams).

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 1

- **Supports:** M1.3

</details>

- [x] Task 4 complete

---

### Task 5: Redirect following is bounded

#### Problem

`request()` follows every 3xx from Reddit by calling itself. There is no hop limit, so a redirect loop keeps one visitor's request running forever, and a 3xx with no `Location` header makes it request the bare API base URL.

#### Solution

- Add `fn redirect_path(location: Option<&HeaderValue>, redirects_left: u8) -> Result<String, String>` holding the existing path rewrite and the new limits.
- `request()` gains a `redirects_left: u8` argument; `reddit_get` and `reddit_short_head` pass `MAX_REDIRECTS` (5) and each recursion passes `redirects_left - 1`.

#### Notes

- **Location:** `src/client.rs`

- **Verify:** `cargo test --locked --lib client::tests::redirect_path` passes both new tests.

- Boundary: pure function; the recursion wiring is checked by review.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — add to `src/client.rs` (current behaviour, no limits):

```rust
const MAX_REDIRECTS: u8 = 5;

fn redirect_path(location: Option<&wreq_header::HeaderValue>, _redirects_left: u8) -> Result<String, String> {
	if location.and_then(|h| h.to_str().ok()) == Some(ALTERNATIVE_REDDIT_URL_BASE) {
		return Err("Reddit response was invalid".to_string());
	}
	Ok(
		location
			.map(|val| {
				let new_path = percent_encode(val.as_bytes(), CONTROLS)
					.to_string()
					.trim_start_matches(REDDIT_URL_BASE)
					.trim_start_matches(ALTERNATIVE_REDDIT_URL_BASE)
					.to_string();
				format!("{new_path}{}raw_json=1", if new_path.contains('?') { "&" } else { "?" })
			})
			.unwrap_or_default(),
	)
}
```

**Test (red)** — add to `mod tests` in `src/client.rs`:

```rust
	#[test]
	fn redirect_path_follows_reddit_locations() {
		let location = wreq_header::HeaderValue::from_static("https://oauth.reddit.com/r/rust/about.json");
		assert_eq!(redirect_path(Some(&location), MAX_REDIRECTS), Ok("/r/rust/about.json?raw_json=1".to_string()));
		let with_query = wreq_header::HeaderValue::from_static("/r/rust/top.json?t=day");
		assert_eq!(redirect_path(Some(&with_query), 1), Ok("/r/rust/top.json?t=day&raw_json=1".to_string()));
	}

	#[test]
	fn redirect_path_rejects_missing_location_and_exhausted_hops() {
		let location = wreq_header::HeaderValue::from_static("/r/rust/about.json");
		assert!(redirect_path(None, MAX_REDIRECTS).is_err(), "a redirect without Location cannot be followed");
		assert!(redirect_path(Some(&location), 0).is_err(), "no redirects remain");
		let homepage = wreq_header::HeaderValue::from_static("https://www.reddit.com");
		assert!(redirect_path(Some(&homepage), MAX_REDIRECTS).is_err());
	}
```

Run `cargo test --locked --lib client::tests::redirect_path`; the second test fails because the stub returns `Ok("")` for a missing `Location` and ignores the hop count.

**Implementation guidance (green)**:

- In `redirect_path`, return `Err("Reddit redirected too many times".to_string())` when `redirects_left == 0`, and `Err("Reddit redirected without a Location header".to_string())` when `location` is `None`; keep the homepage check and rewrite.
- `request(...)`: add `redirects_left: u8` as the last parameter; in the 3xx branch replace the inline `location_header.map(...)` block (`src/client.rs:304-326`) with `let next_path = redirect_path(response.headers().get(wreq::header::LOCATION), redirects_left)?;` then `return request(method, next_path, true, quarantine, base_path, host, redirects_left - 1).await;`.
- `reddit_get` and `reddit_short_head` pass `MAX_REDIRECTS`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 4 (same file; avoids conflicting edits)

- **Supports:** M1.3

</details>

- [x] Task 5 complete

---

### Task 6: Reddit failures get accurate messages and refresh the token only when rate limited

#### Problem

`client::json()` treats every empty body as a rate limit: it shows "Reddit rate limit exceeded" and spawns a token refresh, even on a 200 that says plenty of quota remains. A 403 or 429 with an HTML body shows "Failed to parse page JSON data", which tells the visitor nothing.

#### Solution

- Add `fn upstream_failure(status: u16, body_is_empty: bool, ratelimit_remaining: Option<f32>, reset: Option<&str>) -> Option<(String, bool)>` returning the visitor message and whether to refresh the token.
- Rate limited means status 429 or a remaining quota of zero or less; only that refreshes the token. 403 explains Reddit blocked the request; 5xx keeps the outage message; other empty bodies report "empty response" with the status.
- `json()` calls it for empty bodies and for JSON parse failures. JSON error bodies (private, banned, quarantined) are unchanged.

#### Notes

- **Location:** `src/client.rs`

- **Verify:** `cargo test --locked --lib client::tests::upstream_failure` passes.

- Assumption: Reddit signals rate limits by status 429 or `x-ratelimit-remaining: 0`; a 200 with an empty body and quota left is no longer treated as one.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — add to `src/client.rs`:

```rust
fn upstream_failure(_status: u16, _body_is_empty: bool, _ratelimit_remaining: Option<f32>, _reset: Option<&str>) -> Option<(String, bool)> {
	None
}
```

**Test (red)** — add to `mod tests` in `src/client.rs`:

```rust
	#[test]
	fn upstream_failure_refreshes_the_token_only_when_rate_limited() {
		let (message, refresh) = upstream_failure(429, false, None, Some("42")).expect("429 is a failure");
		assert!(refresh);
		assert!(message.contains("rate limit") && message.contains("42"), "{message}");

		let (_, refresh) = upstream_failure(200, true, Some(0.0), None).expect("an exhausted quota is a failure");
		assert!(refresh);

		let (message, refresh) = upstream_failure(200, true, Some(57.0), None).expect("an empty body is a failure");
		assert!(!refresh, "quota remains, so the token is fine");
		assert!(message.contains("empty") && message.contains("200"), "{message}");
	}

	#[test]
	fn upstream_failure_explains_blocks_and_outages() {
		let (message, refresh) = upstream_failure(403, false, Some(57.0), None).expect("403 HTML is a failure");
		assert!(!refresh);
		assert!(message.contains("403"), "{message}");

		let (message, refresh) = upstream_failure(503, false, None, None).expect("5xx is a failure");
		assert!(!refresh);
		assert!(message.contains("outage"), "{message}");

		assert_eq!(upstream_failure(200, false, Some(57.0), None), None, "a non-empty 200 is a plain parse error");
	}
```

Run `cargo test --locked --lib client::tests::upstream_failure`; both fail on the first `expect` because the stub returns `None`.

**Implementation guidance (green)**:

- Order the checks: rate limited → `(format!("Reddit rate limit exceeded. Try refreshing in a few seconds.{}", reset.map(|r| format!(" Rate limit will reset in: {r}")).unwrap_or_default()), true)`; `403` → `("Reddit blocked this request (HTTP 403). This instance may be rate limited or blocked by Reddit.", false)`; `500..=599` → `("Reddit is having issues, check if there's an outage", false)`; `body_is_empty` → `(format!("Reddit returned an empty response (HTTP {status})"), false)`; otherwise `None`.
- In `json()`, keep the parsed `remaining` as `ratelimit_remaining: Option<f32>` alongside `reset` (`src/client.rs:429-447`).
- Empty body (`:454`): `let (message, refresh) = upstream_failure(status.as_u16(), true, ratelimit_remaining, reset.as_deref()).expect("empty bodies are always failures");` — or `unwrap_or_else` with the empty-response text — then `if refresh { tokio::spawn(force_refresh_token()); }` and `return Err(message)`.
- Parse failure (`:511-517`): if `upstream_failure(status.as_u16(), false, ratelimit_remaining, reset.as_deref())` is `Some((message, refresh))`, spawn the refresh when `refresh` and return `Err(message)`; otherwise keep `err("Failed to parse page JSON data", ...)`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 5 (same file)

- **Supports:** M1.3

</details>

- [x] Task 6 complete

---

### Task 7: GitHub outages return 502 instead of crashing the request

#### Problem

Error pages run `check_update.js`, which asks Redlib for `/commits.atom` and `/instances.json`. Redlib fetches them from GitHub with `.expect()`, so when GitHub is down the handler panics, the visitor's connection resets, and nothing is cached, so the next error page panics again.

#### Solution

- Add `fetch_text(url) -> Result<String, String>` (send, `error_for_status`, read text) and `text_response(body, content_type)` that serves 200 with the body or 502 on error.
- The two cached fetchers return `Result<String, String>` with `#[cached(time = 600, result = true)]`, so only successes are cached.

#### Notes

- **Location:** `src/main.rs`

- **Verify:** `cargo test --locked --bin redlib` passes both new tests.

- Boundary: a closed loopback port stands in for GitHub; the real GitHub URLs are not called.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Stub** — add to `src/main.rs`:

```rust
async fn fetch_text(url: &str) -> Result<String, String> {
	Ok(CLIENT.get(url).send().await.expect("Failed to request GitHub").text().await.expect("Failed to read body"))
}

fn text_response(body: Result<String, String>, content_type: &str) -> Response<Body> {
	Response::builder()
		.status(200)
		.header("content-type", content_type)
		.body(Body::from(body.unwrap_or_default()))
		.unwrap_or_default()
}
```

**Test (red)** — add at the end of `src/main.rs`:

```rust
#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test(flavor = "multi_thread")]
	async fn unreachable_update_source_becomes_bad_gateway() {
		// Bind and immediately release a port so nothing is listening on it.
		let address = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
		let fetched = fetch_text(&format!("http://{address}/commits.atom")).await;
		assert!(fetched.is_err());
		assert_eq!(text_response(fetched, "application/atom+xml").status(), 502);
	}

	#[test]
	fn fetched_text_is_served_with_its_content_type() {
		let response = text_response(Ok("<feed/>".to_string()), "application/atom+xml");
		assert_eq!(response.status(), 200);
		assert_eq!(response.headers()["content-type"], "application/atom+xml");
	}
}
```

Run `cargo test --locked --bin redlib`; `unreachable_update_source_becomes_bad_gateway` panics with "Failed to request GitHub".

**Implementation guidance (green)**:

- `fetch_text`: `CLIENT.get(url).send().await.and_then(|response| response.error_for_status()).map_err(|e| format!("Failed to request {url}: {e}"))?` then `.text().await.map_err(|e| format!("Failed to read {url}: {e}"))`.
- `text_response`: on `Err(e)`, `warn!("{e}")` and return status 502, `content-type: text/plain`, body `"Upstream request failed"`.
- `fetch_commit_info` / `fetch_instances` (`src/main.rs:432-454`): return `Result<String, String>`, call `fetch_text(url).await`, change the attribute to `#[cached(time = 600, result = true)]`.
- `proxy_commit_info` / `proxy_instances` (`src/main.rs:422-447`): `Ok(text_response(fetch_commit_info().await, "application/atom+xml"))` and the JSON equivalent.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 4 (relies on `CLIENT` timeouts so the fetch cannot hang)

- **Supports:** M1.4

</details>

- [x] Task 7 complete

---

### Task 8: Device identifiers and tokens are logged only at trace level

#### Problem

At the default `info` level Redlib logs the spoofed Android device UUID, the web-auth device ID, and the first 32 characters of every bearer token. Operators ship these logs to aggregators where they identify the instance's Reddit session.

#### Solution

- Change the token-retrieved lines (both backends), the Android spoofing line, and the `GenericWebAuth` device-ID line from `info!` to `trace!`. The messages stay available for debugging with `RUST_LOG=trace`.

#### Notes

- **Location:** `src/oauth.rs`

- **Verify:** `cargo test --locked --lib oauth::tests::device_identifiers` passes.

- Boundary: a capturing logger in tests; token lines need live Reddit, so they are covered by review only.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Test (red)** — add to `mod tests` in `src/oauth.rs`:

```rust
	use log::{Level, LevelFilter, Log, Metadata, Record};
	use std::sync::{Mutex, Once};

	struct CapturingLogger(Mutex<Vec<(Level, String)>>);

	impl Log for CapturingLogger {
		fn enabled(&self, _: &Metadata) -> bool {
			true
		}
		fn log(&self, record: &Record) {
			self.0.lock().unwrap().push((record.level(), record.args().to_string()));
		}
		fn flush(&self) {}
	}

	static LOGGER: CapturingLogger = CapturingLogger(Mutex::new(Vec::new()));

	fn captured_logs() -> &'static Mutex<Vec<(Level, String)>> {
		static INSTALL: Once = Once::new();
		INSTALL.call_once(|| {
			let _ = log::set_logger(&LOGGER);
			log::set_max_level(LevelFilter::Trace);
		});
		&LOGGER.0
	}

	#[test]
	fn device_identifiers_are_only_logged_at_trace() {
		let logs = captured_logs();
		let device = Device::android();
		let web = GenericWebAuth::new();
		let identifiers = [device.headers["X-Reddit-Device-Id"].clone(), web.device_id.clone()];

		let logs = logs.lock().unwrap();
		for identifier in &identifiers {
			assert!(
				logs.iter().any(|(level, message)| *level == Level::Trace && message.contains(identifier.as_str())),
				"{identifier} should still be available at trace level"
			);
			assert!(
				!logs.iter().any(|(level, message)| *level < Level::Trace && message.contains(identifier.as_str())),
				"{identifier} must not be logged above trace"
			);
		}
	}
```

Run `cargo test --locked --lib oauth::tests::device_identifiers`; it fails on "should still be available at trace level" because both identifiers are logged at `info`.

**Implementation guidance (green)**:

- `src/oauth.rs:285` and `:398-402`: `info!` → `trace!` for the "Retrieved token" lines.
- `src/oauth.rs:324` and `:453`: `info!` → `trace!`.
- Keep the `info!("[✅] Successfully created OAuth client")` line; it contains no identifier.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 3 (same file)

- **Supports:** M1.6

</details>

- [x] Task 8 complete

---

### Task 9: Integration / Boundary Verification

#### Problem

Slice 1 is accepted only when every milestone (M1.1–M1.6) has a passing regression test, the default suite is offline, and the repository's format and lint checks stay clean. No living `.feature` files exist in this repository, so the executable assertions are the Rust tests named below.

#### Solution

- Run the offline suite, format check, and clippy.
- Confirm each milestone's regression tests ran and passed by name.
- Smoke-test the real binary against live Reddit once, to confirm startup authentication and a page load still work after the OAuth refactor.

#### Notes

- **Location:** whole crate

- **Verify:** all three commands below exit 0, the test output lists every test named in the table as `ok`, and the smoke run returns HTTP 200 for `/r/rust`.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Commands**:

- `cargo fmt --check`
- `cargo clippy --locked --all-targets -- -D warnings`
- `cargo test --locked`
- Live smoke (not part of the regression suite): `cargo run --locked -- --address 127.0.0.1 --port 8080`, then `curl -s -o /dev/null -w "%{http_code}" http://127.0.0.1:8080/r/rust` returns `200`; stop the server with Ctrl+C.

**Milestone evidence**:

| Milestone | Tests that must pass |
| --- | --- |
| M1.1 | `oauth::tests::rollover_guard_allows_one_holder_and_releases_on_drop`, `rollover_guard_releases_when_refresh_panics`, `rollover_guard_releases_when_refresh_future_is_cancelled`, `remember_session_headers_skips_non_ascii_values`, `token_preview_never_panics` |
| M1.2 | `oauth::tests::authentication_gives_up_with_an_error_after_falling_back_to_web_auth`, `authentication_returns_the_first_success` |
| M1.3 | `client::tests::client_times_out_when_upstream_stops_responding`, `redirect_path_follows_reddit_locations`, `redirect_path_rejects_missing_location_and_exhausted_hops`, `upstream_failure_refreshes_the_token_only_when_rate_limited`, `upstream_failure_explains_blocks_and_outages` |
| M1.4 | `tests::unreachable_update_source_becomes_bad_gateway`, `tests::fetched_text_is_served_with_its_content_type` (bin `redlib`) |
| M1.5 | `cargo test --locked` passes with the 18 live tests reported as ignored |
| M1.6 | `oauth::tests::device_identifiers_are_only_logged_at_trace` |

Not verified here: live Reddit rate-limit and redirect behaviour (mock-free pure tests stand in), and token-log levels during real authentication (review only).

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Tasks 1–8

- **Supports:** M1.1, M1.2, M1.3, M1.4, M1.5, M1.6

</details>

- [x] Task 9 complete

## Acceptance Evidence

Slice 1 accepted on 2026-11-28 against baseline `ba116c1`.

- **Checks:** `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D warnings` clean (confirmed it catches a planted violation); `cargo test --locked` passes 63 tests offline with 20 live tests ignored.
- **Red-green:** every regression test in the Task 9 table, plus the five added after review, was observed failing before its fix and passing after.
- **Live smoke:** startup authentication succeeds and `/settings`, `/commits.atom`, `/instances.json` return 200. Reddit returns 403 to this network on both `main` and this branch, so a live page load could not be verified; the 403 now shows "Reddit blocked this request".

### Review

A generalist and an adversarial safety reviewer read the first snapshot, and a third reviewer checked the corrections. No Critical findings. Verified findings and their dispositions:

| Finding | Disposition | Regression test |
| --- | --- | --- |
| A failed runtime refresh let every visitor request start a new ~95 s auth cycle during an outage | Fixed: `roll_over` holds the rollover flag for 60 s after a failure; `force_refresh_token()` returns `RefreshOutcome` | `failed_refresh_keeps_other_refreshes_out_during_its_cooldown`, `roll_over_reports_what_happened` |
| The client-wide 30 s read timeout aborted media streams when a visitor paused | Fixed: `media_get` sets a 1 h read timeout for `/img`, `/vid`, `/hls` | `media_stream_survives_a_paused_reader` |
| The 1 h media read timeout also applied to waiting for response headers | Fixed: `send_media` bounds the header wait at 30 s | `media_request_times_out_waiting_for_headers` |
| A malformed auth response logged the raw JSON (possibly the access token) at `error!` | Fixed: `describe_auth_error` logs the JSON at trace only | `auth_errors_never_include_the_access_token` |
| A GitHub 3xx passed `error_for_status` and would cache an empty feed | Fixed: `fetch_text` requires 2xx | `redirected_update_source_is_an_error` |
| The token daemon could spin on a token lifetime under 120 s, and treated a skipped refresh as success | Fixed: 60 s sleep floor; the daemon retries until `Refreshed` | Review only (daemon needs live Reddit) |

Accepted as review-only (the wiring needs live Reddit or the global `OAUTH_CLIENT`): `force_refresh_token` passing the real cooldown, the spawned 401 refresh, `json()` call sites of `upstream_failure`, `proxy()` using `media_get`/`send_media`, and `request()` decrementing `redirects_left`.

Open, not verified: whether Reddit throttles tokens with 403 or empty 401 bodies (those no longer roll the token), and an extra harmless refresh when the daemon retries after a visitor-triggered refresh succeeded.
