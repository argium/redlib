use crate::{
	client::{CLIENT, OAUTH_CLIENT, OAUTH_IS_ROLLING_OVER, OAUTH_RATELIMIT_REMAINING},
	oauth_resources::ANDROID_APP_VERSION_LIST,
};
use base64::{engine::general_purpose, Engine as _};
use log::{error, info, trace, warn};
use serde_json::json;
use std::{
	collections::HashMap,
	sync::atomic::{AtomicBool, Ordering},
	time::Duration,
};
use tegen::tegen::TextGenerator;
use tokio::time::timeout;

const REDDIT_ANDROID_OAUTH_CLIENT_ID: &str = "ohXpoqrZYub1kg";

const AUTH_ENDPOINT: &str = "https://www.reddit.com";

const OAUTH_TIMEOUT: Duration = Duration::from_secs(5);

// Try the Android backend this many times before falling back to web auth.
const OAUTH_FALLBACK_AFTER_FAILURES: u32 = 5;
const OAUTH_MAX_FAILURES: u32 = 10;

// How long a failed refresh holds off further refresh attempts.
const OAUTH_RETRY_AFTER_FAILED_REFRESH: Duration = Duration::from_secs(60);

// Response from OAuth backend authentication
#[derive(Debug, Clone)]
pub struct OauthResponse {
	pub token: String,
	pub expires_in: u64,
	pub additional_headers: HashMap<String, String>,
}

// Trait for OAuth backend implementations
trait OauthBackend: Send + Sync {
	fn authenticate(&mut self) -> impl std::future::Future<Output = Result<OauthResponse, AuthError>> + Send;
	fn user_agent(&self) -> &str;
	fn get_headers(&self) -> HashMap<String, String>;
}

// OAuth backend implementations
#[derive(Debug, Clone)]
pub(crate) enum OauthBackendImpl {
	MobileSpoof(MobileSpoofAuth),
	GenericWeb(GenericWebAuth),
}

impl OauthBackend for OauthBackendImpl {
	async fn authenticate(&mut self) -> Result<OauthResponse, AuthError> {
		match self {
			OauthBackendImpl::MobileSpoof(backend) => backend.authenticate().await,
			OauthBackendImpl::GenericWeb(backend) => backend.authenticate().await,
		}
	}

	fn user_agent(&self) -> &str {
		match self {
			OauthBackendImpl::MobileSpoof(backend) => backend.user_agent(),
			OauthBackendImpl::GenericWeb(backend) => backend.user_agent(),
		}
	}

	fn get_headers(&self) -> HashMap<String, String> {
		match self {
			OauthBackendImpl::MobileSpoof(backend) => backend.get_headers(),
			OauthBackendImpl::GenericWeb(backend) => backend.get_headers(),
		}
	}
}

// Spoofed client for Android devices
#[derive(Debug, Clone)]
pub struct Oauth {
	pub(crate) headers_map: HashMap<String, String>,
	expires_in: u64,
	pub(crate) backend: OauthBackendImpl,
}

impl Oauth {
	/// Creates the OAuth client at startup, exiting the process if every
	/// backend fails. Runtime refreshes use [`Oauth::try_new`] instead.
	pub(crate) async fn new() -> Self {
		Self::try_new().await.unwrap_or_else(|e| {
			error!("[⛔] Failed to create OAuth client (mobile + generic): {e}");
			std::process::exit(1);
		})
	}

	/// Creates an OAuth client, trying the Android backend first and falling
	/// back to web authentication. Returns the last error once every attempt fails.
	pub(crate) async fn try_new() -> Result<Self, String> {
		let oauth = authenticate_with_fallback(Self::authenticate_once, OAUTH_TIMEOUT).await?;
		info!("[✅] Successfully created OAuth client");
		Ok(oauth)
	}

	async fn authenticate_once(mut backend: OauthBackendImpl) -> Result<Self, String> {
		let attempt = timeout(OAUTH_TIMEOUT, async move {
			let response = backend.authenticate().await?;

			// Build headers_map from backend headers + Authorization header
			let mut headers_map = backend.get_headers();
			headers_map.insert("Authorization".to_owned(), format!("Bearer {}", response.token));
			headers_map.extend(response.additional_headers);

			Ok(Self {
				headers_map,
				expires_in: response.expires_in,
				backend,
			})
		})
		.await;

		match attempt {
			Ok(Ok(oauth)) => Ok(oauth),
			Ok(Err(error)) => Err(describe_auth_error(error)),
			Err(_) => Err("timed out".to_string()),
		}
	}

	pub fn user_agent(&self) -> &str {
		self.backend.user_agent()
	}
}

#[derive(Debug)]
enum AuthError {
	Wreq(wreq::Error),
	SerdeDeserialize(serde_json::Error),
	Field((serde_json::Value, &'static str)),
}

/// Describes an authentication failure for logs. The raw auth response can
/// contain the access token, so it is only logged at trace level.
fn describe_auth_error(error: AuthError) -> String {
	match error {
		AuthError::Wreq(error) => error.to_string(),
		AuthError::SerdeDeserialize(error) => error.to_string(),
		AuthError::Field((value, error)) => {
			trace!("Unexpected OAuth response: {value}");
			format!("unexpected OAuth response: missing or invalid {error}")
		}
	}
}

impl From<wreq::Error> for AuthError {
	fn from(err: wreq::Error) -> Self {
		AuthError::Wreq(err)
	}
}

impl From<serde_json::Error> for AuthError {
	fn from(err: serde_json::Error) -> Self {
		AuthError::SerdeDeserialize(err)
	}
}

pub async fn token_daemon() {
	// Monitor for refreshing token
	loop {
		// Get expiry time - be sure to not hold the read lock
		let expires_in = { OAUTH_CLIENT.load_full().expires_in };

		// Sleep until 2 minutes before expiry, but never spin if Reddit hands
		// out a very short-lived token.
		let duration = Duration::from_secs(expires_in.saturating_sub(120)).max(OAUTH_RETRY_AFTER_FAILED_REFRESH);

		info!("[⏳] Waiting for {duration:?} seconds before refreshing OAuth token...");

		tokio::time::sleep(duration).await;

		info!("[⌛] {duration:?} Elapsed! Refreshing OAuth token...");

		// A failed refresh already waited out its cooldown. If a visitor-triggered
		// refresh is running, check back shortly in case it fails.
		loop {
			match force_refresh_token().await {
				RefreshOutcome::Refreshed => break,
				RefreshOutcome::Failed => {}
				RefreshOutcome::AlreadyRunning => tokio::time::sleep(OAUTH_TIMEOUT).await,
			}
		}
	}
}

/// Runs `attempt` until it succeeds: first with the Android backend, then
/// with web authentication after `OAUTH_FALLBACK_AFTER_FAILURES` failures.
/// Returns the last error after `OAUTH_MAX_FAILURES` failures instead of
/// exiting, so a runtime refresh can keep the current token.
async fn authenticate_with_fallback<T, F, Fut>(mut attempt: F, retry_delay: Duration) -> Result<T, String>
where
	F: FnMut(OauthBackendImpl) -> Fut,
	Fut: std::future::Future<Output = Result<T, String>>,
{
	let mut backend = OauthBackendImpl::MobileSpoof(MobileSpoofAuth::new());
	let mut failures = 0;

	loop {
		let error = match attempt(backend.clone()).await {
			Ok(value) => return Ok(value),
			Err(error) => error,
		};

		failures += 1;
		error!("[⛔] Failed to create OAuth client ({failures}/{OAUTH_MAX_FAILURES}): {error}");
		if failures >= OAUTH_MAX_FAILURES {
			return Err(error);
		}

		if failures == OAUTH_FALLBACK_AFTER_FAILURES && matches!(backend, OauthBackendImpl::MobileSpoof(_)) {
			warn!("[🔄] MobileSpoofAuth failed {failures} times. Falling back to GenericWebAuth...");
			backend = OauthBackendImpl::GenericWeb(GenericWebAuth::new());
		}

		tokio::time::sleep(retry_delay).await;
	}
}

/// Holds the token-rollover flag for one refresh attempt and clears it when
/// dropped, so a cancelled or panicking refresh cannot leave every later
/// refresh skipped until restart.
struct RolloverGuard(&'static AtomicBool);

impl RolloverGuard {
	/// Returns `None` when another refresh already holds the flag.
	fn try_acquire(flag: &'static AtomicBool) -> Option<Self> {
		flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).ok().map(|_| Self(flag))
	}
}

impl Drop for RolloverGuard {
	fn drop(&mut self) {
		self.0.store(false, Ordering::SeqCst);
	}
}

/// Copies Reddit's device-identifying session headers so later token
/// refreshes resend them. Values that are not visible ASCII are skipped.
fn remember_session_headers(response_headers: &wreq::header::HeaderMap, remembered: &mut HashMap<String, String>) {
	for name in ["x-reddit-loid", "x-reddit-session"] {
		if let Some(value) = response_headers.get(name).and_then(|value| value.to_str().ok()) {
			remembered.insert(name.to_owned(), value.to_owned());
		}
	}
}

/// The first 32 characters of a token, for diagnostics.
fn token_preview(token: &str) -> &str {
	token.char_indices().nth(32).map_or(token, |(end, _)| &token[..end])
}

/// What a call to [`force_refresh_token`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshOutcome {
	/// A new client was authenticated and installed.
	Refreshed,
	/// Another refresh held the rollover flag, so nothing was done.
	AlreadyRunning,
	/// Authentication failed; the current client was kept.
	Failed,
}

/// Runs one token refresh while holding `flag`, installing the new client on
/// success. After a failure the flag stays held for `failure_cooldown`, so
/// visitor requests (which spawn refreshes while the rate limit counter stays
/// low) cannot hammer Reddit's auth endpoint during an outage.
async fn roll_over<T>(
	flag: &'static AtomicBool,
	refresh: impl std::future::Future<Output = Result<T, String>>,
	install: impl FnOnce(T),
	failure_cooldown: Duration,
) -> RefreshOutcome {
	let Some(_guard) = RolloverGuard::try_acquire(flag) else {
		trace!("Skipping refresh token roll over, already in progress");
		return RefreshOutcome::AlreadyRunning;
	};
	match refresh.await {
		Ok(client) => {
			install(client);
			RefreshOutcome::Refreshed
		}
		Err(e) => {
			error!("[⛔] OAuth token refresh failed; keeping the current token for at least {failure_cooldown:?}: {e}");
			tokio::time::sleep(failure_cooldown).await;
			RefreshOutcome::Failed
		}
	}
}

/// Replaces the shared OAuth client with a freshly authenticated one. On
/// failure the current client is kept and further refreshes are held off for
/// `OAUTH_RETRY_AFTER_FAILED_REFRESH`.
pub async fn force_refresh_token() -> RefreshOutcome {
	trace!("Rolling over refresh token. Current rate limit: {}", OAUTH_RATELIMIT_REMAINING.load(Ordering::SeqCst));
	roll_over(
		&OAUTH_IS_ROLLING_OVER,
		Oauth::try_new(),
		|new_client| {
			OAUTH_CLIENT.swap(new_client.into());
			OAUTH_RATELIMIT_REMAINING.store(99, Ordering::SeqCst);
		},
		OAUTH_RETRY_AFTER_FAILED_REFRESH,
	)
	.await
}

#[derive(Debug, Clone, Default)]
struct Device {
	oauth_id: String,
	initial_headers: HashMap<String, String>,
	headers: HashMap<String, String>,
	user_agent: String,
}

// MobileSpoofAuth backend - spoofs an Android mobile device
#[derive(Debug, Clone)]
pub struct MobileSpoofAuth {
	device: Device,
	additional_headers: HashMap<String, String>,
}

impl MobileSpoofAuth {
	fn new() -> Self {
		Self {
			device: Device::new(),
			additional_headers: HashMap::new(),
		}
	}
}

impl OauthBackend for MobileSpoofAuth {
	async fn authenticate(&mut self) -> Result<OauthResponse, AuthError> {
		// Construct URL for OAuth token
		let url = format!("{AUTH_ENDPOINT}/auth/v2/oauth/access-token/loid");
		let mut builder = CLIENT.post(&url);

		// Add headers from spoofed client
		for (key, value) in &self.device.initial_headers {
			builder = builder.header(key, value);
		}
		// Set up HTTP Basic Auth - basically just the const OAuth ID's with no password,
		// Base64-encoded. https://en.wikipedia.org/wiki/Basic_access_authentication
		// This could be constant, but I don't think it's worth it. OAuth ID's can change
		// over time and we want to be flexible.
		let auth = general_purpose::STANDARD.encode(format!("{}:", self.device.oauth_id));
		builder = builder.header("Authorization", format!("Basic {auth}"));

		// Set JSON body. I couldn't tell you what this means. But that's what the client sends
		let json = json!({
				"scopes": ["*","email", "pii"]
		});

		trace!("Sending token request to {url}...");

		// Send request
		let resp = builder.json(&json).send().await?;

		trace!("Received response with status {} and length {:?}", resp.status(), resp.headers().get("content-length"));
		trace!("OAuth headers: {:#?}", resp.headers());

		// Parse headers - loid header _should_ be saved sent on subsequent token refreshes.
		// Technically it's not needed, but it's easy for Reddit API to check for this.
		// It's some kind of header that uniquely identifies the device.
		// Not worried about the privacy implications, since this is randomly changed
		// and really only as privacy-concerning as the OAuth token itself.
		remember_session_headers(resp.headers(), &mut self.additional_headers);

		trace!("Serializing response...");

		// Serialize response
		let json: serde_json::Value = resp.json().await?;

		trace!("Accessing relevant fields...");

		// Save token and expiry
		let token = json
			.get("access_token")
			.ok_or_else(|| AuthError::Field((json.clone(), "access_token")))?
			.as_str()
			.ok_or_else(|| AuthError::Field((json.clone(), "access_token: as_str")))?
			.to_string();
		let expires_in = json
			.get("expires_in")
			.ok_or_else(|| AuthError::Field((json.clone(), "expires_in")))?
			.as_u64()
			.ok_or_else(|| AuthError::Field((json.clone(), "expires_in: as_u64")))?;

		trace!("[✅] Success - Retrieved token \"{}...\", expires in {}", token_preview(&token), expires_in);

		Ok(OauthResponse {
			token,
			expires_in,
			additional_headers: self.additional_headers.clone(),
		})
	}

	fn user_agent(&self) -> &str {
		&self.device.user_agent
	}

	fn get_headers(&self) -> HashMap<String, String> {
		let mut headers = self.device.headers.clone();
		headers.extend(self.additional_headers.clone());
		headers
	}
}

// GenericWebAuth backend - simple web-based authentication
#[derive(Debug, Clone)]
pub struct GenericWebAuth {
	device_id: String,
	user_agent: String,
	additional_headers: HashMap<String, String>,
}

impl GenericWebAuth {
	fn new() -> Self {
		// Generate random 20-character alphanumeric device_id
		let device_id: String = (0..20)
			.map(|_| {
				let idx = fastrand::usize(..62);
				let chars = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
				chars[idx] as char
			})
			.collect();

		trace!("[🔄] Using GenericWebAuth with device_id: \"{device_id}\"");

		Self {
			device_id,
			user_agent: fake_user_agent::get_rua().to_owned(),
			additional_headers: HashMap::new(),
		}
	}
}

impl OauthBackend for GenericWebAuth {
	async fn authenticate(&mut self) -> Result<OauthResponse, AuthError> {
		// Construct URL for OAuth token
		let url = "https://www.reddit.com/api/v1/access_token";
		let mut builder = CLIENT.post(url);

		// Add minimal headers
		builder = builder.header("Host", "www.reddit.com");
		builder = builder.header("User-Agent", &self.user_agent);
		builder = builder.header("Accept", "*/*");
		builder = builder.header("Accept-Language", "en-US,en;q=0.5");
		// builder = builder.header("Accept-Encoding", "gzip, deflate, br, zstd");
		builder = builder.header("Authorization", "Basic M1hmQkpXbGlIdnFBQ25YcmZJWWxMdzo=");
		builder = builder.header("Content-Type", "application/x-www-form-urlencoded");
		builder = builder.header("Sec-GPC", "1");
		builder = builder.header("Connection", "keep-alive");

		// Set up form body
		let body_str = format!("grant_type=https%3A%2F%2Foauth.reddit.com%2Fgrants%2Finstalled_client&device_id={}", self.device_id);

		trace!("Sending GenericWebAuth token request to {url}...");

		// Send request
		let resp: wreq::Response = builder.body(body_str).send().await?;

		trace!("Received response with status {} and length {:?}", resp.status(), resp.headers().get("content-length"));
		trace!("GenericWebAuth headers: {:#?}", resp.headers());

		// Parse headers - loid header _should_ be saved sent on subsequent token refreshes.
		// Technically it's not needed, but it's easy for Reddit API to check for this.
		// It's some kind of header that uniquely identifies the device.
		// Not worried about the privacy implications, since this is randomly changed
		// and really only as privacy-concerning as the OAuth token itself.
		remember_session_headers(resp.headers(), &mut self.additional_headers);

		trace!("Serializing GenericWebAuth response...");

		// Serialize response
		let json: serde_json::Value = resp.json().await?;

		trace!("Accessing relevant fields...");

		// Parse response - access_token, token_type, device_id, expires_in, scope
		let token = json
			.get("access_token")
			.ok_or_else(|| AuthError::Field((json.clone(), "access_token")))?
			.as_str()
			.ok_or_else(|| AuthError::Field((json.clone(), "access_token: as_str")))?
			.to_string();
		let expires_in = json
			.get("expires_in")
			.ok_or_else(|| AuthError::Field((json.clone(), "expires_in")))?
			.as_u64()
			.ok_or_else(|| AuthError::Field((json.clone(), "expires_in: as_u64")))?;

		trace!("[✅] GenericWebAuth success - Retrieved token \"{}...\", expires in {}", token_preview(&token), expires_in);

		// Insert a few necessary headers
		self.additional_headers.insert("Origin".to_owned(), "https://www.reddit.com".to_owned());
		self.additional_headers.insert("User-Agent".to_owned(), self.user_agent.to_owned());

		Ok(OauthResponse {
			token,
			expires_in,
			additional_headers: self.additional_headers.clone(),
		})
	}

	fn user_agent(&self) -> &str {
		&self.user_agent
	}

	fn get_headers(&self) -> HashMap<String, String> {
		self.additional_headers.clone()
	}
}

impl Device {
	fn android() -> Self {
		// Generate uuid
		let uuid = uuid::Uuid::new_v4().to_string();

		// Generate random user-agent
		let android_app_version = choose(ANDROID_APP_VERSION_LIST).to_string();
		let android_version = fastrand::u8(9..=14);

		let android_user_agent = format!("Reddit/{android_app_version}/Android {android_version}");

		let qos = fastrand::u32(1000..=100_000);
		let qos: f32 = qos as f32 / 1000.0;
		let qos = format!("{qos:.3}");

		let codecs = TextGenerator::new().generate("available-codecs=video/avc, video/hevc{, video/x-vnd.on2.vp9|}");

		// Android device headers
		let headers: HashMap<String, String> = HashMap::from([
			("User-Agent".into(), android_user_agent.clone()),
			("x-reddit-retry".into(), "algo=no-retries".into()),
			("x-reddit-compression".into(), "1".into()),
			("x-reddit-qos".into(), qos),
			("x-reddit-media-codecs".into(), codecs),
			("Content-Type".into(), "application/json; charset=UTF-8".into()),
			("client-vendor-id".into(), uuid.clone()),
			("X-Reddit-Device-Id".into(), uuid.clone()),
		]);

		trace!("[🔄] Spoofing Android client with headers: {headers:?}, uuid: \"{uuid}\", and OAuth ID \"{REDDIT_ANDROID_OAUTH_CLIENT_ID}\"");

		Self {
			oauth_id: REDDIT_ANDROID_OAUTH_CLIENT_ID.to_string(),
			headers: headers.clone(),
			initial_headers: headers,
			user_agent: android_user_agent,
		}
	}
	fn new() -> Self {
		// See https://github.com/redlib-org/redlib/issues/8
		Self::android()
	}
}

fn choose<T: Copy>(list: &[T]) -> T {
	*fastrand::choose_multiple(list.iter(), 1)[0]
}

#[cfg(test)]
mod tests {
	use super::*;
	use log::{Level, LevelFilter, Log, Metadata, Record};
	use std::sync::{Mutex, Once};

	#[test]
	fn auth_errors_never_include_the_access_token() {
		let response = serde_json::json!({ "access_token": "secret-token-value", "expires_in": "soon" });
		let message = describe_auth_error(AuthError::Field((response, "expires_in: as_u64")));
		assert!(message.contains("expires_in"), "{message}");
		assert!(!message.contains("secret-token-value"), "{message}");
	}

	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "requires live Reddit"]
	async fn test_mobile_spoof_backend() {
		// Test MobileSpoofAuth backend specifically
		let mut backend = MobileSpoofAuth::new();
		let response = backend.authenticate().await;
		assert!(response.is_ok());
		let response = response.unwrap();
		assert!(!response.token.is_empty());
		assert!(response.expires_in > 0);
		assert!(!backend.user_agent().is_empty());
		assert!(!backend.get_headers().is_empty());
	}

	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "todolol"]
	async fn test_generic_web_backend() {
		// Test GenericWebAuth backend specifically
		let mut backend = GenericWebAuth::new();
		let response = backend.authenticate().await;
		assert!(response.is_ok());
		let response = response.unwrap();
		assert!(!response.token.is_empty());
		assert!(response.expires_in > 0);
		assert!(!backend.user_agent().is_empty());
	}

	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "requires live Reddit"]
	async fn test_oauth_client() {
		// Integration test - tests the overall Oauth client
		assert!(OAUTH_CLIENT.load_full().headers_map.contains_key("Authorization"));
	}

	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "requires live Reddit"]
	async fn test_oauth_client_refresh() {
		force_refresh_token().await;
	}

	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "requires live Reddit"]
	async fn test_oauth_token_exists() {
		let client = OAUTH_CLIENT.load_full();
		let auth_header = client.headers_map.get("Authorization").unwrap();
		assert!(auth_header.starts_with("Bearer "));
	}

	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "requires live Reddit"]
	async fn test_oauth_headers_len() {
		assert!(OAUTH_CLIENT.load_full().headers_map.len() >= 3);
	}

	#[test]
	fn test_creating_device() {
		Device::new();
	}

	#[test]
	fn test_creating_backends() {
		// Test that both backends can be created
		MobileSpoofAuth::new();
		GenericWebAuth::new();
	}

	#[test]
	fn rollover_guard_allows_one_holder_and_releases_on_drop() {
		static FLAG: AtomicBool = AtomicBool::new(false);
		let guard = RolloverGuard::try_acquire(&FLAG).expect("first refresh acquires the flag");
		assert!(RolloverGuard::try_acquire(&FLAG).is_none(), "a second refresh must be skipped while one is running");
		drop(guard);
		assert!(!FLAG.load(Ordering::SeqCst));
		assert!(RolloverGuard::try_acquire(&FLAG).is_some(), "a later refresh can run again");
	}

	#[tokio::test]
	async fn failed_refresh_keeps_other_refreshes_out_during_its_cooldown() {
		static FLAG: AtomicBool = AtomicBool::new(false);
		let mut failed = Box::pin(roll_over(
			&FLAG,
			async { Err::<(), _>("reddit unavailable".to_string()) },
			|_| {},
			Duration::from_secs(3600),
		));
		assert!(
			futures_lite::future::poll_once(&mut failed).await.is_none(),
			"a failed refresh waits out its cooldown before releasing the flag"
		);
		assert_eq!(
			roll_over(&FLAG, async { Ok(()) }, |_| {}, Duration::ZERO).await,
			RefreshOutcome::AlreadyRunning,
			"visitor requests must not start another refresh during the cooldown"
		);
		drop(failed);
		assert!(!FLAG.load(Ordering::SeqCst), "cancelling the cooldown still releases the flag");
	}

	#[tokio::test]
	async fn roll_over_reports_what_happened() {
		static FLAG: AtomicBool = AtomicBool::new(false);
		let mut installed = None;
		assert_eq!(
			roll_over(&FLAG, async { Ok(7) }, |client| installed = Some(client), Duration::ZERO).await,
			RefreshOutcome::Refreshed
		);
		assert_eq!(installed, Some(7));
		assert_eq!(
			roll_over(&FLAG, async { Err::<(), _>("reddit unavailable".to_string()) }, |_| {}, Duration::ZERO).await,
			RefreshOutcome::Failed
		);
		assert!(!FLAG.load(Ordering::SeqCst));
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
				async move {
					if attempt < 3 {
						Err("flaky".to_string())
					} else {
						Ok(attempt)
					}
				}
			},
			Duration::ZERO,
		)
		.await;
		assert_eq!(outcome, Ok(3));
	}

	#[test]
	fn token_preview_never_panics() {
		assert_eq!(token_preview("short"), "short");
		assert_eq!(token_preview(&"a".repeat(40)), "a".repeat(32));
		assert_eq!(token_preview(&"é".repeat(40)).chars().count(), 32);
	}

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
}
