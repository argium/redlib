#![allow(dead_code)]
#![allow(clippy::cmp_owned)]

use brotli::enc::{BrotliCompress, BrotliEncoderParams};
use cookie::Cookie;
use futures_lite::{
	future::{self, Boxed},
	Future, FutureExt,
};
use hyper::{
	body,
	body::{Bytes, HttpBody},
	header,
	service::{make_service_fn, service_fn},
	HeaderMap,
};
use hyper::{Body, Method, Request, Response, Server as HyperServer};
use libflate::gzip;
use route_recognizer::{Params, Router};
use std::{
	cmp::Ordering,
	fmt::Display,
	io,
	net::SocketAddr,
	panic::AssertUnwindSafe,
	pin::Pin,
	result::Result,
	str::{from_utf8, Split},
	string::ToString,
	time::{Duration, Instant},
};
use time::OffsetDateTime;

use crate::{config, dbg_msg};
use log::{debug, error, warn};

const BANNED_USER_AGENTS: &[&str] = &[
	"AI2Bot",
	"Ai2Bot-Dolma",
	"Amazonbot",
	"Andibot",
	"Applebot",
	"Applebot-Extended",
	"Awario",
	"Brightbot 1.0",
	"Bytespider",
	"CCBot",
	"ChatGPT-User",
	"Claude-SearchBot",
	"Claude-User",
	"Claude-Web",
	"ClaudeBot",
	"Cotoyogi",
	"Crawlspace",
	"Datenbank Crawler",
	"Devin",
	"Diffbot",
	"DuckAssistBot",
	"Echobot Bot",
	"EchoboxBot",
	"FacebookBot",
	"Factset_spyderbot",
	"FirecrawlAgent",
	"FriendlyCrawler",
	"GPTBot",
	"Google-CloudVertexBot",
	"Google-Extended",
	"GoogleOther",
	"GoogleOther-Image",
	"GoogleOther-Video",
	"ICC-Crawler",
	"ISSCyberRiskCrawler",
	"ImagesiftBot",
	"Kangaroo Bot",
	"Meta-ExternalAgent",
	"Meta-ExternalFetcher",
	"MistralAI-User",
	"MistralAI-User/1.0",
	"MyCentralAIScraperBot",
	"NovaAct",
	"OAI-SearchBot",
	"Operator",
	"PanguBot",
	"Panscient",
	"Perplexity-User",
	"PerplexityBot",
	"PetalBot",
	"PhindBot",
	"Poseidon Research Crawler",
	"QualifiedBot",
	"QuillBot",
	"SBIntuitionsBot",
	"Scrapy",
	"SemrushBot",
	"SemrushBot-BA",
	"SemrushBot-CT",
	"SemrushBot-OCOB",
	"SemrushBot-SI",
	"SemrushBot-SWA",
	"Sidetrade indexer bot",
	"TikTokSpider",
	"Timpibot",
	"VelenPublicWebCrawler",
	"WARDBot",
	"Webzio-Extended",
	"YandexAdditional",
	"YandexAdditionalBot",
	"YouBot",
	"aiHitBot",
	"anthropic-ai",
	"bedrockbot",
	"cohere-ai",
	"cohere-training-data-crawler",
	"facebookexternalhit",
	"iaskspider/2.0",
	"img2dataset",
	"meta-externalagent",
	"meta-externalfetcher",
	"omgili",
	"omgilibot",
	"panscient.com",
	"quillbot.com",
	"wpbot",
];

type BoxResponse = Pin<Box<dyn Future<Output = Result<Response<Body>, String>> + Send>>;

type Handler = fn(Request<Body>) -> BoxResponse;

/// A registered endpoint. `route` is the path template (`/r/:sub/comments/:id`), which names
/// the endpoint in logs without exposing the path a visitor actually requested.
#[derive(Clone)]
struct Endpoint {
	route: String,
	handler: Handler,
}

/// Compressors for the response Body, in ascending order of preference.
#[derive(Copy, Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum CompressionType {
	Passthrough,
	Gzip,
	Brotli,
}

/// All browsers support gzip, so if we are given `Accept-Encoding: *`, deliver
/// gzipped-content.
///
/// Brotli would be nice universally, but Safari (iOS, iPhone, macOS) reportedly
/// doesn't support it yet.
const DEFAULT_COMPRESSOR: CompressionType = CompressionType::Gzip;

/// Smaller bodies fit in one IP frame, so compressing them gains nothing.
const COMPRESSION_MIN_BYTES: usize = 1452;

const BROTLI_QUALITY: i32 = 4;
const BROTLI_WINDOW_BITS: i32 = 20;

impl CompressionType {
	/// Returns a `CompressionType` given a content coding
	/// in [RFC 7231](https://datatracker.ietf.org/doc/html/rfc7231#section-5.3.4)
	/// format.
	fn parse(s: &str) -> Option<Self> {
		let c = match s {
			// Compressors we support.
			"gzip" => Self::Gzip,
			"br" => Self::Brotli,

			// The wildcard means that we can choose whatever
			// compression we prefer. In this case, use the
			// default.
			"*" => DEFAULT_COMPRESSOR,

			// Compressor not supported.
			_ => return None,
		};

		Some(c)
	}
}

impl Display for CompressionType {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Gzip => write!(f, "gzip"),
			Self::Brotli => write!(f, "br"),
			Self::Passthrough => Ok(()),
		}
	}
}

pub struct Route<'a> {
	router: &'a mut Router<Endpoint>,
	path: String,
}

pub struct Server {
	pub default_headers: HeaderMap,
	/// How long a client may take to send the request headers of one request.
	pub header_read_timeout: Duration,
	/// How long open connections may keep running after a shutdown signal before the server stops anyway.
	pub shutdown_deadline: Duration,
	router: Router<Endpoint>,
}

/// Browsers send headers in one packet; this only needs to outlast a slow mobile connection.
const DEFAULT_HEADER_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// Docker's `stop` waits 10 s before SIGKILL; finish inside that so shutdown is clean.
pub const DEFAULT_SHUTDOWN_DEADLINE: Duration = Duration::from_secs(5);

#[macro_export]
macro_rules! headers(
	{ $($key:expr => $value:expr),+ } => {
		{
			let mut m = hyper::HeaderMap::new();
			$(
				if let Ok(val) = hyper::header::HeaderValue::from_str($value) {
					m.insert($key, val);
				}
			)+
			m
		}
	 };
);

pub trait RequestExt {
	fn params(&self) -> Params;
	fn param(&self, name: &str) -> Option<String>;
	fn set_params(&mut self, params: Params) -> Option<Params>;
	fn cookies(&self) -> Vec<Cookie<'_>>;
	fn cookie(&self, name: &str) -> Option<Cookie<'_>>;
}

pub trait ResponseExt {
	fn cookies(&self) -> Vec<Cookie<'_>>;
	fn insert_cookie(&mut self, cookie: Cookie<'_>);
	fn remove_cookie(&mut self, name: String);
}

impl RequestExt for Request<Body> {
	fn params(&self) -> Params {
		self.extensions().get::<Params>().unwrap_or(&Params::new()).clone()
		// self.extensions()
		// 	.get::<RequestMeta>()
		// 	.and_then(|meta| meta.route_params())
		// 	.expect("Routerify: No RouteParams added while processing request")
	}

	fn param(&self, name: &str) -> Option<String> {
		self.params().find(name).map(std::borrow::ToOwned::to_owned)
	}

	fn set_params(&mut self, params: Params) -> Option<Params> {
		self.extensions_mut().insert(params)
	}

	fn cookies(&self) -> Vec<Cookie<'_>> {
		self.headers().get("Cookie").map_or(Vec::new(), |header| {
			header
				.to_str()
				.unwrap_or_default()
				.split("; ")
				.map(|cookie| Cookie::parse(cookie).unwrap_or_else(|_| Cookie::from("")))
				.collect()
		})
	}

	fn cookie(&self, name: &str) -> Option<Cookie<'_>> {
		self.cookies().into_iter().find(|c| c.name() == name)
	}
}

impl ResponseExt for Response<Body> {
	fn cookies(&self) -> Vec<Cookie<'_>> {
		self.headers().get("Cookie").map_or(Vec::new(), |header| {
			header
				.to_str()
				.unwrap_or_default()
				.split("; ")
				.map(|cookie| Cookie::parse(cookie).unwrap_or_else(|_| Cookie::from("")))
				.collect()
		})
	}

	fn insert_cookie(&mut self, cookie: Cookie<'_>) {
		if let Ok(val) = header::HeaderValue::from_str(&cookie.to_string()) {
			self.headers_mut().append("Set-Cookie", val);
		}
	}

	fn remove_cookie(&mut self, name: String) {
		let removal_cookie = Cookie::build(name).path("/").http_only(true).expires(OffsetDateTime::now_utc());
		if let Ok(val) = header::HeaderValue::from_str(&removal_cookie.to_string()) {
			self.headers_mut().append("Set-Cookie", val);
		}
	}
}

impl Route<'_> {
	fn method(&mut self, method: &Method, dest: Handler) -> &mut Self {
		let endpoint = Endpoint {
			route: self.path.clone(),
			handler: dest,
		};
		self.router.add(&format!("/{}{}", method.as_str(), self.path), endpoint);
		self
	}

	/// Add an endpoint for `GET` requests
	pub fn get(&mut self, dest: fn(Request<Body>) -> BoxResponse) -> &mut Self {
		self.method(&Method::GET, dest)
	}

	/// Add an endpoint for `POST` requests
	pub fn post(&mut self, dest: fn(Request<Body>) -> BoxResponse) -> &mut Self {
		self.method(&Method::POST, dest)
	}
}

impl Default for Server {
	fn default() -> Self {
		Self::new()
	}
}

impl Server {
	pub fn new() -> Self {
		Self {
			default_headers: HeaderMap::new(),
			header_read_timeout: DEFAULT_HEADER_READ_TIMEOUT,
			shutdown_deadline: DEFAULT_SHUTDOWN_DEADLINE,
			router: Router::new(),
		}
	}

	pub fn at(&mut self, path: &str) -> Route<'_> {
		Route {
			path: path.to_owned(),
			router: &mut self.router,
		}
	}

	/// Binds `addr` and serves until Ctrl+C or SIGTERM. The address is bound before this
	/// returns, so a port that is already in use shows up as an `Err` from the future
	/// rather than a panic.
	pub fn listen(self, addr: SocketAddr) -> Boxed<Result<(), hyper::Error>> {
		self.listen_until(addr, shutdown_signal())
	}

	/// Like [`Server::listen`], but stops when `shutdown` resolves. After that, open
	/// connections get `shutdown_deadline` to finish before the future returns anyway, so a
	/// stalled client cannot keep the process alive.
	pub fn listen_until(self, addr: SocketAddr, shutdown: impl Future<Output = ()> + Send + 'static) -> Boxed<Result<(), hyper::Error>> {
		let header_read_timeout = self.header_read_timeout;
		let shutdown_deadline = self.shutdown_deadline;
		let make_svc = make_service_fn(move |_conn| {
			// For correct borrowing, these values need to be borrowed
			let router = self.router.clone();
			let default_headers = self.default_headers.clone();

			// This is the `Service` that will handle the connection.
			// `service_fn` is a helper to convert a function that
			// returns a Response into a `Service`.
			// let shared_router = router.clone();
			async move {
				Ok::<_, String>(service_fn(move |req: Request<Body>| {
					let started = Instant::now();
					let method = req.method().clone();
					let req_headers = req.headers().clone();
					let def_headers = default_headers.clone();

					// Catch robots.txt-disrespecful bots who still identify themselves
					// Typically justified as "human triggered" actions.
					if match config::get_setting("REDLIB_ROBOTS_DISABLE_INDEXING") {
						Some(val) => val == "on",
						None => false,
					} {
						if let Some(user_agent) = req_headers.get("user-agent") {
							if let Ok(user_agent_str) = user_agent.to_str() {
								for banned in BANNED_USER_AGENTS {
									if user_agent_str.contains(banned) {
										let response = new_boilerplate(def_headers, req_headers, 403, Body::from("Forbidden"));
										return logged_response(method, "blocked-user-agent".to_string(), started, response.boxed());
									}
								}
							}
						}
					}

					// Remove double slashes and decode encoded slashes
					let mut path = req.uri().path().replace("//", "/").replace("%2F", "/");

					// Remove trailing slashes
					if path != "/" && path.ends_with('/') {
						path.pop();
					}

					// Replace HEAD with GET for routing
					let (route_method, is_head) = match req.method() {
						&Method::HEAD => (&Method::GET, true),
						method => (method, false),
					};

					// Match the visited path with an added route
					match router.recognize(&format!("/{}{}", route_method.as_str(), path)) {
						// If a route was configured for this path
						Ok(found) => {
							let endpoint = Endpoint::clone(found.handler());
							let route = endpoint.route.clone();
							let mut parammed = req;
							parammed.set_params(found.params().clone());

							let response = serve_route(endpoint, parammed, method.clone(), is_head, req_headers, def_headers);
							logged_response(method, route, started, response.boxed())
						}
						// If there was a routing error
						Err(e) => {
							let response = new_boilerplate(def_headers, req_headers, 404, if is_head { Body::empty() } else { e.into() });
							logged_response(method, "unmatched".to_string(), started, response.boxed())
						}
					}
				}))
			}
		});

		let builder = match HyperServer::try_bind(&addr) {
			Ok(builder) => builder,
			Err(e) => return future::ready(Err(e)).boxed(),
		};

		// The header timeout drops connections that trickle in their headers (slowloris); hyper
		// runs it on the Tokio timer.
		let (shutdown_started, shutdown_started_rx) = tokio::sync::oneshot::channel();
		let server = builder.http1_header_read_timeout(header_read_timeout).serve(make_svc).with_graceful_shutdown(async move {
			shutdown.await;
			let _ = shutdown_started.send(());
		});

		async move {
			// hyper's graceful shutdown waits for every open connection with no limit, so race it
			// against the deadline that starts when shutdown does.
			let deadline = async {
				if shutdown_started_rx.await.is_ok() {
					tokio::time::sleep(shutdown_deadline).await;
				} else {
					future::pending::<()>().await;
				}
			};
			tokio::select! {
				result = server => result,
				() = deadline => {
					warn!("Shutdown deadline of {shutdown_deadline:?} reached with connections still open; exiting");
					Ok(())
				}
			}
		}
		.boxed()
	}
}

/// Resolves on Ctrl+C or, on Unix, SIGTERM (what `docker stop` sends).
async fn shutdown_signal() {
	#[cfg(windows)]
	// Wait for the CTRL+C signal
	tokio::signal::ctrl_c().await.expect("Failed to install CTRL+C signal handler");

	#[cfg(unix)]
	{
		let mut signal_terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("Failed to install SIGTERM signal handler");
		tokio::select! {
			_ = tokio::signal::ctrl_c() => (),
			_ = signal_terminate.recv() => ()
		}
	}
}

/// Runs one route's handler and turns the outcome into the response to send.
///
/// A handler that panics (while being called or while its future runs) or returns `Err`
/// becomes a `500` instead of a dropped connection. This is the one place those failures
/// are logged. Only the method and route template are recorded: the `Err` message can
/// echo a visitor's search terms, so it stays at debug level, and the default panic hook
/// has already printed the panic message.
async fn serve_route(
	endpoint: Endpoint,
	req: Request<Body>,
	method: Method,
	is_head: bool,
	req_headers: HeaderMap<header::HeaderValue>,
	def_headers: HeaderMap<header::HeaderValue>,
) -> Result<Response<Body>, String> {
	let Endpoint { route, handler } = endpoint;
	let outcome = AssertUnwindSafe(async move { handler(req).await }).catch_unwind().await;

	match outcome {
		Ok(Ok(mut res)) => {
			res.headers_mut().extend(def_headers);
			if is_head {
				*res.body_mut() = Body::empty();
			} else {
				let _ = compress_response(&req_headers, &mut res).await;
			}

			Ok(res)
		}
		Ok(Err(msg)) => {
			// Do not log `msg` even at debug: transport errors (such as wreq media failures)
			// embed the upstream URI, which carries the visitor's path and query.
			warn!("{method} {route} handler returned an error");
			new_boilerplate(def_headers, req_headers, 500, if is_head { Body::empty() } else { Body::from(msg) }).await
		}
		Err(_panic) => {
			error!("{method} {route} handler panicked");
			let body = if is_head { Body::empty() } else { Body::from("Internal Server Error") };
			new_boilerplate(def_headers, req_headers, 500, body).await
		}
	}
}

/// Awaits `response`, then logs one debug line: method, route template, status and duration.
/// Never log the path, query string, headers or client address, which can identify a visitor.
fn logged_response(method: Method, route: String, started: Instant, response: Boxed<Result<Response<Body>, String>>) -> Boxed<Result<Response<Body>, String>> {
	async move {
		let result = response.await;
		let elapsed_ms = started.elapsed().as_millis();
		match &result {
			Ok(res) => debug!("{method} {route} {} {elapsed_ms}ms", res.status().as_u16()),
			Err(_) => debug!("{method} {route} failed before a response was built {elapsed_ms}ms"),
		}
		result
	}
	.boxed()
}

/// Create a boilerplate Response for error conditions. This response will be
/// compressed if requested by client.
async fn new_boilerplate(
	default_headers: HeaderMap<header::HeaderValue>,
	req_headers: HeaderMap<header::HeaderValue>,
	status: u16,
	body: Body,
) -> Result<Response<Body>, String> {
	match Response::builder().status(status).body(body) {
		Ok(mut res) => {
			let _ = compress_response(&req_headers, &mut res).await;

			res.headers_mut().extend(default_headers.clone());
			Ok(res)
		}
		Err(msg) => Err(msg.to_string()),
	}
}

/// Determines the desired compressor based on the Accept-Encoding header.
///
/// This function will honor the [q-value](https://developer.mozilla.org/en-US/docs/Glossary/Quality_values)
///  for each compressor. The q-value is an optional parameter, a decimal value
/// on \[0..1\], to order the compressors by preference. An Accept-Encoding value
/// with no q-values is also accepted.
///
/// Here are [examples](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Accept-Encoding#examples)
/// of valid Accept-Encoding headers.
///
/// ```http
/// Accept-Encoding: gzip
/// Accept-Encoding: gzip, compress, br
/// Accept-Encoding: br;q=1.0, gzip;q=0.8, *;q=0.1
/// ```
///
/// The result is deliberately not cached: the header comes straight from the
/// client, so a cache keyed on it grows with every distinct value.
fn determine_compressor(accept_encoding: String) -> Option<CompressionType> {
	if accept_encoding.is_empty() {
		return None;
	};

	// Keep track of the compressor candidate based on both the client's
	// preference and our own. Concrete examples:
	//
	// 1. "Accept-Encoding: gzip, br" => assuming we like brotli more than
	//    gzip, and the browser supports brotli, we choose brotli
	//
	// 2. "Accept-Encoding: gzip;q=0.8, br;q=0.3" => the client has stated a
	//    preference for gzip over brotli, so we choose gzip
	//
	// To do this, we need to define a struct which contains the requested
	// requested compressor (abstracted as a CompressionType enum) and the
	// q-value. If no q-value is defined for the compressor, we assume one of
	// 1.0. We first compare compressor candidates by comparing q-values, and
	// then CompressionTypes. We keep track of whatever is the greatest per our
	// ordering.

	struct CompressorCandidate {
		alg: CompressionType,
		q: f64,
	}

	impl Ord for CompressorCandidate {
		fn cmp(&self, other: &Self) -> Ordering {
			// Compare q-values. Break ties with the
			// CompressionType values.

			match self.q.total_cmp(&other.q) {
				Ordering::Equal => self.alg.cmp(&other.alg),
				ord => ord,
			}
		}
	}

	impl PartialOrd for CompressorCandidate {
		fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
			Some(self.cmp(other))
		}
	}

	impl PartialEq for CompressorCandidate {
		fn eq(&self, other: &Self) -> bool {
			(self.q == other.q) && (self.alg == other.alg)
		}
	}

	impl Eq for CompressorCandidate {}

	// This is the current candidate.
	//
	// Assmume no candidate so far. We do this by assigning the sentinel value
	// of negative infinity to the q-value. If this value is negative infinity,
	// that means there was no viable compressor candidate.
	let mut cur_candidate = CompressorCandidate {
		alg: CompressionType::Passthrough,
		q: f64::NEG_INFINITY,
	};

	// This loop reads the requested compressors and keeps track of whichever
	// one has the highest priority per our heuristic.
	for val in accept_encoding.split(',') {
		let mut q: f64 = 1.0;

		// The compressor and q-value (if the latter is defined)
		// will be delimited by semicolons.
		let mut spl: Split<'_, char> = val.split(';');

		// Get the compressor. For example, in
		//   gzip;q=0.8
		// this grabs "gzip" in the string. It
		// will further validate the compressor against the
		// list of those we support. If it is not supported,
		// we move onto the next one.
		let compressor: CompressionType = match spl.next() {
			// CompressionType::parse will return the appropriate enum given
			// a string. For example, it will return CompressionType::Gzip
			// when given "gzip".
			Some(s) => match CompressionType::parse(s.trim()) {
				Some(candidate) => candidate,

				// We don't support the requested compression algorithm.
				None => continue,
			},

			// We should never get here, but I'm paranoid.
			None => continue,
		};

		// Get the q-value. This might not be defined, in which case assume
		// 1.0.
		if let Some(s) = spl.next() {
			if !(s.len() > 2 && s.starts_with("q=")) {
				// If the q-value is malformed, the header is malformed, so
				// abort.
				return None;
			}

			match s[2..].parse::<f64>() {
				Ok(val) => {
					if (0.0..=1.0).contains(&val) {
						q = val;
					} else {
						// If the value is outside [0..1], header is malformed.
						// Abort.
						return None;
					};
				}
				Err(_) => {
					// If this isn't a f64, then assume a malformed header
					// value and abort.
					return None;
				}
			}
		};

		// If new_candidate > cur_candidate, make new_candidate the new
		// cur_candidate. But do this safely! It is very possible that
		// someone gave us the string "NAN", which (&str).parse::<f64>
		// will happily translate to f64::NAN.
		let new_candidate = CompressorCandidate { alg: compressor, q };
		if let Some(ord) = new_candidate.partial_cmp(&cur_candidate) {
			if ord == Ordering::Greater {
				cur_candidate = new_candidate;
			}
		};
	}

	if cur_candidate.q == f64::NEG_INFINITY {
		None
	} else {
		Some(cur_candidate.alg)
	}
}

/// The best compressor the request's `Accept-Encoding` allows, if any.
fn requested_compressor(req_headers: &HeaderMap<header::HeaderValue>) -> Option<CompressionType> {
	let accept_encoding = match req_headers.get(header::ACCEPT_ENCODING)?.to_str() {
		Ok(value) => value.to_string(),
		Err(e) => {
			dbg_msg!(e);
			return None;
		}
	};
	determine_compressor(accept_encoding)
}

/// Compress the response body, if possible or desirable. The Body will be
/// compressed in place, and a new header Content-Encoding will be set
/// indicating the compression algorithm.
///
/// This function deems Body eligible compression if and only if the following
/// conditions are met:
///
/// 1. the HTTP client requests a compression encoding in the Content-Encoding
///    header (hence the need for the `req_headers`);
///
/// 2. the content encoding corresponds to a compression algorithm we support;
///
/// 3. the Media type in the Content-Type response header is text with any
///    subtype (e.g. text/plain) or application/json.
///
/// `compress_response` returns Ok on successful compression, or if not all three
/// conditions above are met. It returns Err if there was a problem decoding
/// any header in either `req_headers` or res, but res will remain intact.
///
/// This function logs errors to stderr, but only in debug mode. No information
/// is logged in release builds.
async fn compress_response(req_headers: &HeaderMap<header::HeaderValue>, res: &mut Response<Body>) -> Result<(), String> {
	// Check if the data is eligible for compression.
	if let Some(hdr) = res.headers().get(header::CONTENT_TYPE) {
		match from_utf8(hdr.as_bytes()) {
			Ok(val) => {
				let s = val.to_string();

				// TODO: better determination of what is eligible for compression
				if !(s.starts_with("text/") || s.starts_with("application/json")) {
					return Ok(());
				};
			}
			Err(e) => {
				dbg_msg!(e);
				return Err(e.to_string());
			}
		};
	} else {
		// Response declares no Content-Type. Assume for simplicity that it
		// cannot be compressed.
		return Ok(());
	};

	// Don't bother if the size of the size of the response body will fit
	// within an IP frame (less the bytes that make up the TCP/IP and HTTP
	// headers).
	if res.body().size_hint().lower() < COMPRESSION_MIN_BYTES as u64 {
		return Ok(());
	};

	// A body that was encoded up front (see `StaticBody`) is already final.
	if res.headers().contains_key(header::CONTENT_ENCODING) {
		return Ok(());
	}

	// From here on the representation depends on the client's Accept-Encoding, so
	// shared caches must key on it, whether or not this client gets a compressed body.
	res.headers_mut().append(header::VARY, header::HeaderValue::from_static("Accept-Encoding"));

	let compressor: CompressionType = match requested_compressor(req_headers) {
		Some(c) => c,
		None => return Ok(()),
	};

	compress_body_in_place(res, compressor).await
}

/// A body that never changes between requests, compressed once up front instead of on every
/// request. Compressing a 412 KB script per request is what the per-body cache used to hide.
pub struct StaticBody {
	plain: Bytes,
	gzip: Option<Bytes>,
	brotli: Option<Bytes>,
}

impl StaticBody {
	/// Compresses `plain` now, on the calling thread. Bodies too small to be worth compressing, or
	/// that fail to compress, are served as they are.
	pub fn new(plain: impl Into<Bytes>) -> Self {
		let plain: Bytes = plain.into();
		let encode = |compressor| {
			if plain.len() < COMPRESSION_MIN_BYTES {
				return None;
			}
			compress_body_blocking(compressor, &plain).ok().map(Bytes::from)
		};
		Self {
			gzip: encode(CompressionType::Gzip),
			brotli: encode(CompressionType::Brotli),
			plain,
		}
	}

	/// Puts the body in `res` in the best encoding `req_headers` accepts and sets `Content-Encoding`
	/// and `Vary`. `compress_response` leaves a response that already has a `Content-Encoding` alone.
	pub fn apply(&self, req_headers: &HeaderMap<header::HeaderValue>, res: &mut Response<Body>) {
		let (encoding, body) = match requested_compressor(req_headers) {
			Some(CompressionType::Brotli) if self.brotli.is_some() => (Some(CompressionType::Brotli), self.brotli.clone()),
			Some(CompressionType::Gzip) if self.gzip.is_some() => (Some(CompressionType::Gzip), self.gzip.clone()),
			_ => (None, None),
		};
		let headers = res.headers_mut();
		headers.append(header::VARY, header::HeaderValue::from_static("Accept-Encoding"));
		headers.remove(header::CONTENT_LENGTH);
		if let Some(encoding) = encoding {
			headers.insert(
				header::CONTENT_ENCODING,
				header::HeaderValue::from_static(if encoding == CompressionType::Brotli { "br" } else { "gzip" }),
			);
		}
		*res.body_mut() = Body::from(body.unwrap_or_else(|| self.plain.clone()));
	}
}

/// Replaces the body of `res` with its compressed form and sets `Content-Encoding`.
///
/// Compression runs on the blocking thread pool so it cannot stall the async workers
/// that serve other requests.
async fn compress_body_in_place(res: &mut Response<Body>, compressor: CompressionType) -> Result<(), String> {
	let body_bytes = match body::to_bytes(res.body_mut()).await {
		Ok(bytes) => bytes,
		Err(e) => {
			dbg_msg!(e);
			return Err(e.to_string());
		}
	};

	let compressed = match tokio::task::spawn_blocking(move || compress_body_blocking(compressor, &body_bytes)).await {
		Ok(result) => result,
		Err(e) => Err(e.to_string()),
	};

	match compressed {
		Ok(compressed) => {
			// Replace the body with the compressed payload and set the matching
			// Content-Encoding. Any precomputed Content-Length is no longer valid.
			let headers = res.headers_mut();
			headers.insert(header::CONTENT_ENCODING, compressor.to_string().parse().unwrap());
			headers.remove(header::CONTENT_LENGTH);

			*(res.body_mut()) = Body::from(compressed);
			Ok(())
		}
		Err(e) => Err(e),
	}
}

/// Compresses `body_bytes` with `compressor`. CPU-bound: call it from a blocking
/// thread, not from an async task. Nothing is cached; dynamic pages rarely repeat
/// byte for byte, so keying a cache on whole bodies cost more than it saved.
fn compress_body_blocking(compressor: CompressionType, body_bytes: &[u8]) -> Result<Vec<u8>, String> {
	// io::Cursor implements io::Read, required for our encoders.
	let mut reader = io::Cursor::new(body_bytes);

	let compressed: Vec<u8> = match compressor {
		CompressionType::Gzip => {
			let mut gz: gzip::Encoder<Vec<u8>> = match gzip::Encoder::new(Vec::new()) {
				Ok(gz) => gz,
				Err(e) => {
					dbg_msg!(e);
					return Err(e.to_string());
				}
			};

			match io::copy(&mut reader, &mut gz) {
				Ok(_) => match gz.finish().into_result() {
					Ok(compressed) => compressed,
					Err(e) => {
						dbg_msg!(e);
						return Err(e.to_string());
					}
				},
				Err(e) => {
					dbg_msg!(e);
					return Err(e.to_string());
				}
			}
		}

		CompressionType::Brotli => {
			// The default quality (11) takes tens of milliseconds per page. Quality 4
			// is several times faster for a modest loss in size.
			let brotli_params = BrotliEncoderParams {
				quality: BROTLI_QUALITY,
				lgwin: BROTLI_WINDOW_BITS,
				..BrotliEncoderParams::default()
			};

			let mut compressed = Vec::<u8>::new();
			match BrotliCompress(&mut reader, &mut compressed, &brotli_params) {
				Ok(_) => compressed,
				Err(e) => {
					dbg_msg!(e);
					return Err(e.to_string());
				}
			}
		}

		// This arm is for any requested compressor for which we don't yet
		// have an implementation.
		CompressionType::Passthrough => {
			let msg = "unsupported compressor".to_string();
			return Err(msg);
		}
	};

	Ok(compressed)
}

#[cfg(test)]
mod tests {
	use super::*;
	use brotli::Decompressor as BrotliDecompressor;
	use lipsum::lipsum;
	use std::{boxed::Box, io};

	#[test]
	fn test_determine_compressor() {
		// Single compressor given.
		assert_eq!(determine_compressor("unsupported".to_string()), None);
		assert_eq!(determine_compressor("gzip".to_string()), Some(CompressionType::Gzip));
		assert_eq!(determine_compressor("*".to_string()), Some(DEFAULT_COMPRESSOR));

		// Multiple compressors.
		assert_eq!(determine_compressor("gzip, br".to_string()), Some(CompressionType::Brotli));
		assert_eq!(determine_compressor("gzip;q=0.8, br;q=0.3".to_string()), Some(CompressionType::Gzip));
		assert_eq!(determine_compressor("br, gzip".to_string()), Some(CompressionType::Brotli));
		assert_eq!(determine_compressor("br;q=0.3, gzip;q=0.4".to_string()), Some(CompressionType::Gzip));

		// Invalid q-values.
		assert_eq!(determine_compressor("gzip;q=NAN".to_string()), None);
	}

	#[test]
	fn dos_and_shortlink_limits_accept_encoding_variants_are_parsed_independently() {
		// Header parsing keeps no state between calls, so a stream of distinct values cannot grow memory.
		// (Before the fix a cache held one entry per distinct header; 500 variants left 500 entries.)
		for variant in 0..500 {
			assert_eq!(determine_compressor(format!("gzip;q=0.{variant}, br;q=0.{variant}")), Some(CompressionType::Brotli));
		}
	}

	#[tokio::test]
	async fn healthz_and_shutdown_stalled_connection_does_not_block_shutdown() {
		use tokio::io::AsyncWriteExt;

		let address = free_address();
		let mut server = Server::new();
		server.header_read_timeout = Duration::from_secs(60);
		server.shutdown_deadline = Duration::from_millis(200);
		let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
		let running = tokio::spawn(server.listen_until(address, async {
			let _ = stopped.await;
		}));

		// A request that never ends its headers keeps the connection busy, so hyper waits for it.
		let mut stalled = tokio::net::TcpStream::connect(address).await.unwrap();
		stalled.write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n").await.unwrap();
		// A completed request proves the server has accepted the connection before shutdown starts.
		assert_eq!(get(address, "/nothing", "").await.0, 404);

		stop.send(()).unwrap();
		let finished = tokio::time::timeout(Duration::from_secs(5), running).await;
		assert!(finished.is_ok(), "shutdown waited for a stalled connection");
		assert!(finished.unwrap().unwrap().is_ok());
	}

	#[tokio::test]
	async fn startup_config_and_bind_occupied_port_is_an_error() {
		let occupied = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
		let address = occupied.local_addr().unwrap();
		let result = Server::new().listen(address).await;
		assert!(result.is_err(), "binding an occupied port must fail with an error the caller can report");
	}

	#[tokio::test]
	async fn http1_header_timeout_closes_slow_clients() {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};

		// Bind and release a port so the server can claim it.
		let address = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
		let mut server = Server::new();
		server.header_read_timeout = Duration::from_millis(200);
		let running = tokio::spawn(server.listen(address));

		// A request line and one header, but never the blank line that ends the headers.
		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		client.write_all(b"GET / HTTP/1.1\r\nHost: example.com\r\n").await.unwrap();

		let mut received = Vec::new();
		let closed = tokio::time::timeout(Duration::from_secs(5), client.read_to_end(&mut received)).await;
		running.abort();
		assert!(closed.is_ok(), "the server kept a connection open that never finished its headers");
	}

	/// Sends one `GET` over a raw socket and returns the status code and the body. Panics when the
	/// server closes the connection without answering, which is what a handler panic used to do.
	async fn get(address: SocketAddr, path: &str, extra_headers: &str) -> (u16, String) {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};

		let mut client = tokio::net::TcpStream::connect(address).await.unwrap();
		let request = format!("GET {path} HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n{extra_headers}\r\n");
		client.write_all(request.as_bytes()).await.unwrap();
		let mut response = Vec::new();
		client.read_to_end(&mut response).await.ok();
		let response = String::from_utf8_lossy(&response).into_owned();
		let status = response
			.strip_prefix("HTTP/1.1 ")
			.and_then(|rest| rest.get(..3))
			.and_then(|code| code.parse().ok())
			.unwrap_or_else(|| panic!("no HTTP response for {path}: {response:?}"));
		let body = response.split_once("\r\n\r\n").map(|(_, body)| body.to_string()).unwrap_or_default();
		(status, body)
	}

	/// A port that was free a moment ago; the server binds it before the first request.
	fn free_address() -> SocketAddr {
		std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap()
	}

	fn ok_route(_: Request<Body>) -> BoxResponse {
		Box::pin(async { Ok(Response::builder().status(200).body(Body::from("fine")).unwrap()) })
	}

	fn sync_panic_route(_: Request<Body>) -> BoxResponse {
		panic!("route panicked while being called")
	}

	fn async_panic_route(_: Request<Body>) -> BoxResponse {
		Box::pin(async { panic!("route panicked while being polled") })
	}

	#[tokio::test]
	async fn panic_and_render_errors_panicking_route_returns_500() {
		let address = free_address();
		let mut server = Server::new();
		server.at("/ok").get(ok_route);
		server.at("/sync-panic").get(sync_panic_route);
		server.at("/async-panic").get(async_panic_route);
		let running = tokio::spawn(server.listen(address));

		assert_eq!(get(address, "/sync-panic", "").await.0, 500);
		assert_eq!(get(address, "/async-panic", "").await.0, 500);
		assert_eq!(get(address, "/ok", "").await, (200, "fine".to_string()), "the server must keep serving after a panic");
		running.abort();
	}

	#[tokio::test]
	async fn request_logs_record_the_route_template_without_personal_data() {
		use crate::test_support::captured_logs;

		// Install the capturing logger before the server logs anything.
		let logs = captured_logs();
		let address = free_address();
		let mut server = Server::new();
		server.at("/r/:sub/comments/:id").get(ok_route);
		let running = tokio::spawn(server.listen(address));

		let cookie_header = "Cookie: theme=secretcookie\r\n";
		assert_eq!(get(address, "/r/privatesub/comments/abc123?q=secretterm", cookie_header).await.0, 200);
		assert_eq!(get(address, "/no/such/route/secretpath", "").await.0, 404);
		running.abort();

		let logs = logs.lock().unwrap();
		let request_lines: Vec<&String> = logs
			.iter()
			.filter(|(level, line)| *level == log::Level::Debug && line.starts_with("GET "))
			.map(|(_, line)| line)
			.collect();
		let route_line = request_lines
			.iter()
			.find(|line| line.contains("/r/:sub/comments/:id"))
			.unwrap_or_else(|| panic!("no request log line was emitted for the route template: {request_lines:?}"));
		assert!(route_line.contains(" 200 ") && route_line.ends_with("ms"), "status and duration expected: {route_line}");
		assert!(request_lines.iter().any(|line| line.contains("unmatched") && line.contains(" 404 ")), "{request_lines:?}");
		for line in &request_lines {
			for secret in ["privatesub", "abc123", "secretterm", "secretcookie", "secretpath", "127.0.0.1"] {
				assert!(!line.contains(secret), "log line leaks {secret}: {line}");
			}
		}
	}

	fn err_route(_: Request<Body>) -> BoxResponse {
		Box::pin(async { Err("error sending request for url (https://i.redd.it/secretsub.jpg?q=hiddenterm)".to_string()) })
	}

	#[tokio::test]
	async fn request_logs_handler_errors_leave_out_the_visitors_path() {
		use crate::test_support::captured_logs;

		let logs = captured_logs();
		let address = free_address();
		let mut server = Server::new();
		server.at("/fails").get(err_route);
		let running = tokio::spawn(server.listen(address));

		assert_eq!(get(address, "/fails", "").await.0, 500);
		running.abort();

		let logs = logs.lock().unwrap();
		let error_lines: Vec<&String> = logs.iter().filter(|(_, line)| line.contains("GET /fails handler")).map(|(_, line)| line).collect();
		assert!(!error_lines.is_empty(), "handler error should still be logged: {logs:?}");
		for line in &error_lines {
			assert!(
				!line.contains("secretsub") && !line.contains("hiddenterm") && !line.contains("i.redd.it"),
				"log line leaks the request path or URI: {line}"
			);
		}
	}

	fn static_body_fixture() -> (StaticBody, &'static [u8]) {
		let plain: &'static [u8] = Box::leak(lipsum(5000).into_bytes().into_boxed_slice());
		(StaticBody::new(plain), plain)
	}

	fn headers_accepting(encoding: &str) -> HeaderMap {
		let mut headers = HeaderMap::new();
		headers.insert(header::ACCEPT_ENCODING, header::HeaderValue::from_str(encoding).unwrap());
		headers
	}

	#[tokio::test]
	async fn static_js_caching_precompressed_body_is_reused_across_requests() {
		let (body, plain) = static_body_fixture();
		let mut encoded_bodies = Vec::new();
		for _ in 0..2 {
			let mut res = Response::builder().status(200).header(header::CONTENT_TYPE, "text/javascript").body(Body::empty()).unwrap();
			body.apply(&headers_accepting("br"), &mut res);
			assert_eq!(res.headers()[header::CONTENT_ENCODING], "br");
			assert_eq!(res.headers()[header::VARY], "Accept-Encoding");
			// What the service does next must not compress again.
			compress_response(&headers_accepting("br"), &mut res).await.unwrap();
			encoded_bodies.push(body::to_bytes(res.body_mut()).await.unwrap());
		}
		assert_eq!(
			encoded_bodies[0].as_ptr(),
			encoded_bodies[1].as_ptr(),
			"the second request compressed the asset again instead of reusing the first result"
		);
		let mut decoded = Vec::new();
		io::copy(&mut BrotliDecompressor::new(io::Cursor::new(encoded_bodies[0].to_vec()), 4096), &mut decoded).unwrap();
		assert!(decoded == plain, "the precompressed body must decode to the original");
	}

	#[tokio::test]
	async fn static_js_caching_precompressed_body_follows_accept_encoding() {
		let (body, plain) = static_body_fixture();
		let respond = |accept: Option<&str>| {
			let mut res = Response::builder().status(200).body(Body::empty()).unwrap();
			body.apply(&accept.map_or_else(HeaderMap::new, headers_accepting), &mut res);
			res
		};

		let mut gzipped = respond(Some("gzip"));
		assert_eq!(gzipped.headers()[header::CONTENT_ENCODING], "gzip");
		let mut decoded = Vec::new();
		io::copy(
			&mut gzip::Decoder::new(io::Cursor::new(body::to_bytes(gzipped.body_mut()).await.unwrap().to_vec())).unwrap(),
			&mut decoded,
		)
		.unwrap();
		assert!(decoded == plain);

		let mut identity = respond(None);
		assert!(identity.headers().get(header::CONTENT_ENCODING).is_none());
		assert_eq!(identity.headers()[header::VARY], "Accept-Encoding");
		assert!(body::to_bytes(identity.body_mut()).await.unwrap() == plain);
	}

	#[tokio::test]
	async fn static_js_caching_compress_response_leaves_encoded_bodies_alone() {
		let mut res = Response::builder()
			.status(200)
			.header(header::CONTENT_TYPE, "text/javascript")
			.header(header::CONTENT_ENCODING, "br")
			.body(Body::from(vec![7u8; 4000]))
			.unwrap();
		compress_response(&headers_accepting("gzip"), &mut res).await.unwrap();
		assert_eq!(res.headers()[header::CONTENT_ENCODING], "br", "an encoded body must not be encoded twice");
		assert_eq!(body::to_bytes(res.body_mut()).await.unwrap().len(), 4000);
	}

	#[tokio::test(flavor = "current_thread")]
	async fn compress_response_does_not_block_the_async_executor() {
		use std::sync::atomic::{AtomicBool, Ordering};

		let other_task_ran = AtomicBool::new(false);
		let mut req_headers = HeaderMap::new();
		req_headers.insert(header::ACCEPT_ENCODING, header::HeaderValue::from_static("br"));
		let mut res = Response::builder()
			.status(200)
			.header(header::CONTENT_TYPE, "text/plain")
			.body(Body::from(lipsum(10000)))
			.unwrap();

		// Compression that runs inline finishes during its first poll, before the second future is polled.
		let compression = async {
			compress_response(&req_headers, &mut res).await.unwrap();
			other_task_ran.load(Ordering::SeqCst)
		};
		let other_task = async { other_task_ran.store(true, Ordering::SeqCst) };
		let (other_task_ran_during_compression, ()) = tokio::join!(compression, other_task);

		assert!(other_task_ran_during_compression, "compression ran inline on the async executor");
		assert_eq!(res.headers()[header::CONTENT_ENCODING], "br");
	}

	#[tokio::test]
	async fn compress_response_sets_vary_for_compressible_responses() {
		fn response(content_type: &str, size: usize) -> Response<Body> {
			Response::builder()
				.status(200)
				.header(header::CONTENT_TYPE, content_type)
				.body(Body::from("a".repeat(size)))
				.unwrap()
		}
		let mut gzip_request = HeaderMap::new();
		gzip_request.insert(header::ACCEPT_ENCODING, header::HeaderValue::from_static("gzip"));

		let mut compressed = response("text/html", 4000);
		compress_response(&gzip_request, &mut compressed).await.unwrap();
		assert_eq!(compressed.headers()[header::CONTENT_ENCODING], "gzip");
		assert_eq!(compressed.headers()[header::VARY], "Accept-Encoding");

		// Without Accept-Encoding the body goes out as is, but a cache must still key it on the header.
		let mut identity = response("text/html", 4000);
		compress_response(&HeaderMap::new(), &mut identity).await.unwrap();
		assert!(identity.headers().get(header::CONTENT_ENCODING).is_none());
		assert_eq!(identity.headers()[header::VARY], "Accept-Encoding");

		for mut never_compressed in [response("text/html", 100), response("image/png", 4000)] {
			compress_response(&gzip_request, &mut never_compressed).await.unwrap();
			assert!(never_compressed.headers().get(header::VARY).is_none());
		}
	}

	#[tokio::test]
	async fn test_compress_response() {
		// This macro generates an Accept-Encoding header value given any number of
		// compressors.
		macro_rules! ae_gen {
			($x:expr) => {
				$x.to_string().as_str()
			};

			($x:expr, $($y:expr),+) => {
				format!("{}, {}", $x.to_string(), ae_gen!($($y),+)).as_str()
			};
		}

		for accept_encoding in [
			"*",
			ae_gen!(CompressionType::Gzip),
			ae_gen!(CompressionType::Brotli, CompressionType::Gzip),
			ae_gen!(CompressionType::Brotli),
		] {
			// Determine what the expected encoding should be based on both the
			// specific encodings we accept.
			let expected_encoding: CompressionType = match determine_compressor(accept_encoding.to_string()) {
				Some(s) => s,
				None => panic!("determine_compressor(accept_encoding.to_string()) => None"),
			};

			// Build headers with our Accept-Encoding.
			let mut req_headers = HeaderMap::new();
			req_headers.insert(header::ACCEPT_ENCODING, header::HeaderValue::from_str(accept_encoding).unwrap());

			// Build test response.
			let lorem_ipsum: String = lipsum(10000);
			let expected_lorem_ipsum = Vec::<u8>::from(lorem_ipsum.as_str());
			let mut res = Response::builder()
				.status(200)
				.header(header::CONTENT_TYPE, "text/plain")
				.body(Body::from(lorem_ipsum))
				.unwrap();

			// Perform the compression.
			if let Err(e) = compress_response(&req_headers, &mut res).await {
				panic!("compress_response(&req_headers, &mut res) => Err(\"{e}\")");
			};

			// If the content was compressed, we expect the Content-Encoding
			// header to be modified.
			assert_eq!(
				res
					.headers()
					.get(header::CONTENT_ENCODING)
					.unwrap_or_else(|| panic!("missing content-encoding header"))
					.to_str()
					.unwrap_or_else(|_| panic!("failed to convert Content-Encoding header::HeaderValue to String")),
				expected_encoding.to_string()
			);

			// Decompress body and make sure it's equal to what we started
			// with.
			//
			// In the case of no compression, just make sure the "new" body in
			// the Response is the same as what with which we start.
			let body_vec = match body::to_bytes(res.body_mut()).await {
				Ok(b) => b.to_vec(),
				Err(e) => panic!("{e}"),
			};

			if expected_encoding == CompressionType::Passthrough {
				assert!(body_vec.eq(&expected_lorem_ipsum));
				continue;
			}

			// This provides an io::Read for the underlying body.
			let mut body_cursor: io::Cursor<Vec<u8>> = io::Cursor::new(body_vec);

			// Match the appropriate decompresor for the given
			// expected_encoding.
			let mut decoder: Box<dyn io::Read> = match expected_encoding {
				CompressionType::Gzip => match gzip::Decoder::new(&mut body_cursor) {
					Ok(dgz) => Box::new(dgz),
					Err(e) => panic!("{e}"),
				},

				CompressionType::Brotli => Box::new(BrotliDecompressor::new(body_cursor, expected_lorem_ipsum.len())),

				_ => panic!("no decompressor for {expected_encoding}"),
			};

			let mut decompressed = Vec::<u8>::new();
			if let Err(e) = io::copy(&mut decoder, &mut decompressed) {
				panic!("{e}");
			};

			assert!(decompressed.eq(&expected_lorem_ipsum));
		}
	}
}
