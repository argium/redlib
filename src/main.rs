// Global specifiers
#![forbid(unsafe_code)]
#![allow(clippy::cmp_owned)]

use cached::proc_macro::cached;
use clap::{Arg, ArgAction, ArgMatches, Command};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::process::ExitCode;
use std::sync::LazyLock;

use futures_lite::FutureExt;
use hyper::{header::HeaderValue, Body, Request, Response};
use log::{info, warn};
use redlib::client::{canonical_path, proxy, rate_limit_check, CLIENT};
use redlib::server::{self, RequestExt};
use redlib::utils::{error, redirect, ThemeAssets};
use redlib::{config, duplicates, headers, instance_info, post, search, settings, subreddit, user};

use redlib::client::OAUTH_CLIENT;

// Create Services

// Required for the manifest to be valid
async fn pwa_logo() -> Result<Response<Body>, String> {
	Ok(
		Response::builder()
			.status(200)
			.header("content-type", "image/png")
			.body(include_bytes!("../static/logo.png").as_ref().into())
			.unwrap_or_default(),
	)
}

// Required for iOS App Icons
async fn iphone_logo() -> Result<Response<Body>, String> {
	Ok(
		Response::builder()
			.status(200)
			.header("content-type", "image/png")
			.body(include_bytes!("../static/apple-touch-icon.png").as_ref().into())
			.unwrap_or_default(),
	)
}

async fn favicon() -> Result<Response<Body>, String> {
	Ok(
		Response::builder()
			.status(200)
			.header("content-type", "image/vnd.microsoft.icon")
			.header("Cache-Control", "public, max-age=1209600, s-maxage=86400")
			.body(include_bytes!("../static/favicon.ico").as_ref().into())
			.unwrap_or_default(),
	)
}

async fn font() -> Result<Response<Body>, String> {
	Ok(
		Response::builder()
			.status(200)
			.header("content-type", "font/woff2")
			.header("Cache-Control", "public, max-age=1209600, s-maxage=86400")
			.body(include_bytes!("../static/Inter.var.woff2").as_ref().into())
			.unwrap_or_default(),
	)
}

async fn opensearch() -> Result<Response<Body>, String> {
	Ok(
		Response::builder()
			.status(200)
			.header("content-type", "application/opensearchdescription+xml")
			.header("Cache-Control", "public, max-age=1209600, s-maxage=86400")
			.body(include_bytes!("../static/opensearch.xml").as_ref().into())
			.unwrap_or_default(),
	)
}

async fn resource(body: &str, content_type: &str, cache: bool) -> Result<Response<Body>, String> {
	let mut res = Response::builder()
		.status(200)
		.header("content-type", content_type)
		.body(body.to_string().into())
		.unwrap_or_default();

	if cache {
		if let Ok(val) = HeaderValue::from_str("public, max-age=1209600, s-maxage=86400") {
			res.headers_mut().insert("Cache-Control", val);
		}
	}

	Ok(res)
}

async fn style() -> Result<Response<Body>, String> {
	let mut res = include_str!("../static/style.css").to_string();
	for file in ThemeAssets::iter() {
		res.push('\n');
		let theme = ThemeAssets::get(file.as_ref()).unwrap();
		res.push_str(std::str::from_utf8(theme.data.as_ref()).unwrap());
	}
	Ok(
		Response::builder()
			.status(200)
			.header("content-type", "text/css")
			.header("Cache-Control", "public, max-age=1209600, s-maxage=86400")
			.body(res.to_string().into())
			.unwrap_or_default(),
	)
}

/// Parses a boolean flag value. An empty value (`IPV4_ONLY=` from a compose file whose
/// host variable is unset) counts as false; anything but a boolean word is an error.
fn parse_flag(value: &str) -> Result<bool, String> {
	match value.to_ascii_lowercase().as_str() {
		"" | "n" | "no" | "f" | "false" | "off" | "0" => Ok(false),
		"y" | "yes" | "t" | "true" | "on" | "1" => Ok(true),
		_ => Err(format!("`{value}` is not a boolean (use true, false, 1, 0, on, off, yes or no)")),
	}
}

/// Builds the command line. `-4`/`-6` also read `IPV4_ONLY`/`IPV6_ONLY`, which must be
/// a boolean (see [`parse_flag`]).
fn cli() -> Command {
	Command::new("Redlib")
		.version(env!("CARGO_PKG_VERSION"))
		.about("Private front-end for Reddit written in Rust ")
		.arg(
			Arg::new("ipv4-only")
				.short('4')
				.long("ipv4-only")
				.env("IPV4_ONLY")
				.value_parser(parse_flag)
				.help("Listen on IPv4 only")
				.action(ArgAction::SetTrue),
		)
		.arg(
			Arg::new("ipv6-only")
				.short('6')
				.long("ipv6-only")
				.env("IPV6_ONLY")
				.value_parser(parse_flag)
				.help("Listen on IPv6 only")
				.action(ArgAction::SetTrue),
		)
		.arg(
			Arg::new("redirect-https")
				.short('r')
				.long("redirect-https")
				.help("Redirect all HTTP requests to HTTPS (no longer functional)")
				.num_args(0),
		)
		.arg(
			Arg::new("address")
				.short('a')
				.long("address")
				.value_name("ADDRESS")
				.help("Sets address to listen on [default: [::], or 0.0.0.0 with -4]")
				.num_args(1),
		)
		.arg(
			Arg::new("port")
				.short('p')
				.long("port")
				.value_name("PORT")
				.env("PORT")
				.help("Port to listen on")
				.default_value("8080")
				.action(ArgAction::Set)
				.num_args(1),
		)
		.arg(
			Arg::new("hsts")
				.short('H')
				.long("hsts")
				.value_name("EXPIRE_TIME")
				.help("HSTS header to tell browsers that this site should only be accessed over HTTPS")
				.default_value("604800")
				.num_args(1),
		)
}

#[tokio::main]
async fn main() -> ExitCode {
	// Load environment variables
	_ = dotenvy::dotenv();

	// Initialize logger
	pretty_env_logger::init();

	// Parse arguments first so `--help` and `--version` work with a broken redlib.toml.
	let matches = cli().get_matches();

	// A broken redlib.toml must stop startup before anything else happens.
	info!("Evaluating config.");
	LazyLock::force(&config::CONFIG);

	match run(&matches).await {
		Ok(()) => ExitCode::SUCCESS,
		Err(message) => {
			eprintln!("{message}");
			ExitCode::FAILURE
		}
	}
}

async fn run(matches: &ArgMatches) -> Result<(), String> {
	let listener = listener_address(matches)?;

	match rate_limit_check().await {
		Ok(()) => {
			info!("[✅] Rate limit check passed");
		}
		Err(e) => {
			let mut message = format!("Rate limit check failed: {e}");
			message += "\nThis may cause issues with the rate limit.";
			message += "\nPlease report this error with the above information.";
			message += "\nhttps://github.com/redlib-org/redlib/issues/new?assignees=sigaloid&labels=bug&title=%F0%9F%90%9B+Bug+Report%3A+Rate+limit+mismatch";
			warn!("{}", message);
			eprintln!("{message}");
		}
	}

	let hsts = matches.get_one("hsts").map(|m: &String| m.as_str());

	println!("Starting Redlib...");

	// Begin constructing a server
	let mut app = server::Server::new();

	// Force evaluation of statics. In instance_info case, we need to evaluate
	// the timestamp so deploy date is accurate - in config case, we need to
	// evaluate the configuration to avoid paying penalty at first request -
	// in OAUTH case, we need to retrieve the token to avoid paying penalty
	// at first request

	info!("Evaluating instance info.");
	LazyLock::force(&instance_info::INSTANCE_INFO);
	info!("Creating OAUTH client.");
	LazyLock::force(&OAUTH_CLIENT);

	// Define default headers (added to all responses)
	app.default_headers = headers! {
		"Referrer-Policy" => "no-referrer",
		"X-Content-Type-Options" => "nosniff",
		"X-Frame-Options" => "DENY",
		"Content-Security-Policy" => "default-src 'none'; font-src 'self'; script-src 'self' blob:; manifest-src 'self'; media-src 'self' data: blob: about:; style-src 'self' 'unsafe-inline'; base-uri 'none'; img-src 'self' data:; form-action 'self'; frame-ancestors 'none'; connect-src 'self'; worker-src blob:;"
	};

	if let Some(expire_time) = hsts {
		if let Ok(val) = HeaderValue::from_str(&format!("max-age={expire_time}")) {
			app.default_headers.insert("Strict-Transport-Security", val);
		}
	}

	// Read static files
	app.at("/style.css").get(|_| style().boxed());
	app
		.at("/manifest.json")
		.get(|_| resource(include_str!("../static/manifest.json"), "application/json", false).boxed());
	app.at("/robots.txt").get(|_| {
		resource(
			if match config::get_setting("REDLIB_ROBOTS_DISABLE_INDEXING") {
				Some(val) => val == "on",
				None => false,
			} {
				"User-agent: *\nDisallow: /"
			} else {
				"User-agent: *\nDisallow: /u/\nDisallow: /user/"
			},
			"text/plain",
			true,
		)
		.boxed()
	});
	app.at("/favicon.ico").get(|_| favicon().boxed());
	app.at("/logo.png").get(|_| pwa_logo().boxed());
	app.at("/Inter.var.woff2").get(|_| font().boxed());
	app.at("/touch-icon-iphone.png").get(|_| iphone_logo().boxed());
	app.at("/apple-touch-icon.png").get(|_| iphone_logo().boxed());
	app.at("/opensearch.xml").get(|_| opensearch().boxed());
	app
		.at("/playHLSVideo.js")
		.get(|_| resource(include_str!("../static/playHLSVideo.js"), "text/javascript", false).boxed());
	app
		.at("/hls.min.js")
		.get(|_| resource(include_str!("../static/hls.min.js"), "text/javascript", false).boxed());
	app
		.at("/highlighted.js")
		.get(|_| resource(include_str!("../static/highlighted.js"), "text/javascript", false).boxed());
	app
		.at("/check_update.js")
		.get(|_| resource(include_str!("../static/check_update.js"), "text/javascript", false).boxed());
	app.at("/copy.js").get(|_| resource(include_str!("../static/copy.js"), "text/javascript", false).boxed());

	app.at("/commits.atom").get(|_| async move { proxy_commit_info().await }.boxed());
	app.at("/instances.json").get(|_| async move { proxy_instances().await }.boxed());

	// Proxy media through Redlib
	app.at("/vid/:id/:size").get(|r| proxy(r, "https://v.redd.it/{id}/DASH_{size}").boxed());
	app.at("/hls/:id/*path").get(|r| proxy(r, "https://v.redd.it/{id}/{path}").boxed());
	app.at("/img/*path").get(|r| proxy(r, "https://i.redd.it/{path}").boxed());
	app.at("/thumb/:point/:id").get(|r| proxy(r, "https://{point}.thumbs.redditmedia.com/{id}").boxed());
	app.at("/emoji/:id/:name").get(|r| proxy(r, "https://emoji.redditmedia.com/{id}/{name}").boxed());
	app
		.at("/emote/:subreddit_id/:filename")
		.get(|r| proxy(r, "https://reddit-econ-prod-assets-permanent.s3.amazonaws.com/asset-manager/{subreddit_id}/{filename}").boxed());
	app
		.at("/preview/:loc/award_images/:fullname/:id")
		.get(|r| proxy(r, "https://{loc}view.redd.it/award_images/{fullname}/{id}").boxed());
	app.at("/preview/:loc/:id").get(|r| proxy(r, "https://{loc}view.redd.it/{id}").boxed());
	app.at("/style/*path").get(|r| proxy(r, "https://styles.redditmedia.com/{path}").boxed());
	app.at("/static/*path").get(|r| proxy(r, "https://www.redditstatic.com/{path}").boxed());

	// Browse user profile
	app
		.at("/u/:name")
		.get(|r| async move { Ok(redirect(&format!("/user/{}", r.param("name").unwrap_or_default()))) }.boxed());
	app.at("/u/:name/comments/:id/:title").get(|r| post::item(r).boxed());
	app.at("/u/:name/comments/:id/:title/:comment_id").get(|r| post::item(r).boxed());

	app.at("/user/[deleted]").get(|req| error(req, "User has deleted their account").boxed());
	app.at("/user/:name.rss").get(|r| user::rss(r).boxed());
	app.at("/user/:name").get(|r| user::profile(r).boxed());
	app.at("/user/:name/:listing").get(|r| user::profile(r).boxed());
	app.at("/user/:name/comments/:id").get(|r| post::item(r).boxed());
	app.at("/user/:name/comments/:id/:title").get(|r| post::item(r).boxed());
	app.at("/user/:name/comments/:id/:title/:comment_id").get(|r| post::item(r).boxed());

	// Configure settings
	app.at("/settings").get(|r| settings::get(r).boxed()).post(|r| settings::set(r).boxed());
	app.at("/settings/restore").get(|r| settings::restore(r).boxed());
	app.at("/settings/encoded-restore").post(|r| settings::encoded_restore(r).boxed());
	app.at("/settings/update").get(|r| settings::update(r).boxed());

	// RSS Subscriptions
	app.at("/r/:sub.rss").get(|r| subreddit::rss(r).boxed());

	// Subreddit services
	app
		.at("/r/:sub")
		.get(|r| subreddit::community(r).boxed())
		.post(|r| subreddit::add_quarantine_exception(r).boxed());

	app
		.at("/r/u_:name")
		.get(|r| async move { Ok(redirect(&format!("/user/{}", r.param("name").unwrap_or_default()))) }.boxed());

	app.at("/r/:sub/subscribe").post(|r| subreddit::subscriptions_filters(r).boxed());
	app.at("/r/:sub/unsubscribe").post(|r| subreddit::subscriptions_filters(r).boxed());
	app.at("/r/:sub/filter").post(|r| subreddit::subscriptions_filters(r).boxed());
	app.at("/r/:sub/unfilter").post(|r| subreddit::subscriptions_filters(r).boxed());

	app.at("/r/:sub/comments/:id").get(|r| post::item(r).boxed());
	app.at("/r/:sub/comments/:id/:title").get(|r| post::item(r).boxed());
	app.at("/r/:sub/comments/:id/:title/:comment_id").get(|r| post::item(r).boxed());
	app.at("/comments/:id").get(|r| post::item(r).boxed());
	app.at("/comments/:id/comments").get(|r| post::item(r).boxed());
	app.at("/comments/:id/comments/:comment_id").get(|r| post::item(r).boxed());
	app.at("/comments/:id/:title").get(|r| post::item(r).boxed());
	app.at("/comments/:id/:title/:comment_id").get(|r| post::item(r).boxed());

	app.at("/r/:sub/duplicates/:id").get(|r| duplicates::item(r).boxed());
	app.at("/r/:sub/duplicates/:id/:title").get(|r| duplicates::item(r).boxed());
	app.at("/duplicates/:id").get(|r| duplicates::item(r).boxed());
	app.at("/duplicates/:id/:title").get(|r| duplicates::item(r).boxed());

	app.at("/r/:sub/search").get(|r| search::find(r).boxed());

	app
		.at("/r/:sub/w")
		.get(|r| async move { Ok(redirect(&format!("/r/{}/wiki", r.param("sub").unwrap_or_default()))) }.boxed());
	app
		.at("/r/:sub/w/*page")
		.get(|r| async move { Ok(redirect(&format!("/r/{}/wiki/{}", r.param("sub").unwrap_or_default(), r.param("wiki").unwrap_or_default()))) }.boxed());
	app.at("/r/:sub/wiki").get(|r| subreddit::wiki(r).boxed());
	app.at("/r/:sub/wiki/*page").get(|r| subreddit::wiki(r).boxed());

	app.at("/r/:sub/about/sidebar").get(|r| subreddit::sidebar(r).boxed());

	app.at("/r/:sub/:sort").get(|r| subreddit::community(r).boxed());

	// Front page
	app.at("/").get(|r| subreddit::community(r).boxed());

	// View Reddit wiki
	app.at("/w").get(|_| async { Ok(redirect("/wiki")) }.boxed());
	app
		.at("/w/*page")
		.get(|r| async move { Ok(redirect(&format!("/wiki/{}", r.param("page").unwrap_or_default()))) }.boxed());
	app.at("/wiki").get(|r| subreddit::wiki(r).boxed());
	app.at("/wiki/*page").get(|r| subreddit::wiki(r).boxed());

	// Search all of Reddit
	app.at("/search").get(|r| search::find(r).boxed());

	// Handle about pages
	app.at("/about").get(|req| error(req, "About pages aren't added yet").boxed());

	// Instance info page
	app.at("/info").get(|r| instance_info::instance_info(r).boxed());
	app.at("/info.:extension").get(|r| instance_info::instance_info(r).boxed());

	// Handle obfuscated share links.
	// Note that this still forces the server to follow the share link to get to the post, so maybe this wants to be updated with a warning before it follow it
	app.at("/r/:sub/s/:id").get(|req| share_link(req).boxed());

	app.at("/:id").get(|req| short_link(req).boxed());

	// Default service in case no routes match
	app.at("/*").get(|req| error(req, "Nothing here").boxed());

	println!("Running Redlib v{} on {listener}!", env!("CARGO_PKG_VERSION"));

	let server = app.listen(listener);

	// Run this server for... forever!
	server.await.map_err(|e| format!("Server error: {e}"))
}

/// Where to listen, from the command line (and `IPV4_ONLY`/`IPV6_ONLY`/`PORT`).
fn listener_address(matches: &ArgMatches) -> Result<SocketAddr, String> {
	resolve_listener(
		matches.get_one::<String>("address").map(String::as_str),
		matches.get_one::<String>("port").expect("port has a default value"),
		matches.get_flag("ipv4-only"),
		matches.get_flag("ipv6-only"),
	)
}

/// Decides the socket address to bind. Without `address` the wildcard address of
/// the chosen family is used (IPv6, which also accepts IPv4 on most systems, unless
/// `-4`). An explicit `address` is never replaced, and must match the family flag.
fn resolve_listener(address: Option<&str>, port: &str, ipv4_only: bool, ipv6_only: bool) -> Result<SocketAddr, String> {
	if ipv4_only && ipv6_only {
		return Err("--ipv4-only (-4) and --ipv6-only (-6) cannot be used together".to_string());
	}
	let port: u16 = port.parse().map_err(|_| format!("Cannot parse {port} as a port (expected 0-65535)"))?;

	let ip = match address {
		Some(address) => {
			let ip: IpAddr = address
				.trim_start_matches('[')
				.trim_end_matches(']')
				.parse()
				.map_err(|_| format!("Cannot parse {address} as an IP address (examples: 0.0.0.0, [::], 127.0.0.1)"))?;
			if ipv4_only && ip.is_ipv6() {
				return Err(format!("--ipv4-only (-4) conflicts with the IPv6 address {address}"));
			}
			if ipv6_only && ip.is_ipv4() {
				return Err(format!("--ipv6-only (-6) conflicts with the IPv4 address {address}"));
			}
			ip
		}
		None if ipv4_only => IpAddr::V4(Ipv4Addr::UNSPECIFIED),
		None => IpAddr::V6(Ipv6Addr::UNSPECIFIED),
	};
	Ok(SocketAddr::new(ip, port))
}

/// Reddit's short post ids are base-36. Anything else (`/Login`, `/ab.cde`) is
/// not a post, so it must not cost an upstream request.
fn is_short_link_id(id: &str) -> bool {
	(5..8).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

/// Share-link ids are mixed-case alphanumeric.
fn is_share_link_id(id: &str) -> bool {
	(8..12).contains(&id.len()) && id.bytes().all(|byte| byte.is_ascii_alphanumeric())
}

/// `/r/:sub/s/:id`: resolves an obfuscated share link to the post's canonical path.
async fn share_link(req: Request<Body>) -> Result<Response<Body>, String> {
	let sub = req.param("sub").unwrap_or_default();
	match req.param("id").as_deref() {
		// Share link
		Some(id) if is_share_link_id(id) => match canonical_path(format!("/r/{sub}/s/{id}"), 3).await {
			Ok(Some(path)) => Ok(redirect(&path)),
			Ok(None) => error(req, "Post ID is invalid. It may point to a post on a community that has been banned.").await,
			Err(e) => error(req, &e).await,
		},

		// Error message for unknown pages
		_ => error(req, "Nothing here").await,
	}
}

/// `/:id`: a front-page sort (`/top`) or a short link to a post (`/1abc23`).
async fn short_link(req: Request<Body>) -> Result<Response<Body>, String> {
	match req.param("id").as_deref() {
		// Sort front page
		Some("best" | "hot" | "new" | "top" | "rising" | "controversial") => subreddit::community(req).await,

		// Short link for post
		Some(id) if is_short_link_id(id) => match canonical_path(format!("/comments/{id}"), 3).await {
			Ok(Some(path)) => Ok(redirect(&path)),
			Ok(None) => error(req, "Post ID is invalid. It may point to a post on a community that has been banned.").await,
			Err(e) => error(req, &e).await,
		},

		// Error message for unknown pages
		_ => error(req, "Nothing here").await,
	}
}

pub async fn proxy_commit_info() -> Result<Response<Body>, String> {
	Ok(text_response(fetch_commit_info().await, "application/atom+xml"))
}

#[cached(time = 600, result = true)]
async fn fetch_commit_info() -> Result<String, String> {
	fetch_text("https://github.com/redlib-org/redlib/commits/main.atom").await
}

pub async fn proxy_instances() -> Result<Response<Body>, String> {
	Ok(text_response(fetch_instances().await, "application/json"))
}

#[cached(time = 600, result = true)]
async fn fetch_instances() -> Result<String, String> {
	fetch_text("https://raw.githubusercontent.com/redlib-org/redlib-instances/refs/heads/main/instances.json").await
}

/// Fetches `url` as text, returning an error instead of panicking when the
/// remote is unreachable or answers with anything but success. `CLIENT` does
/// not follow redirects, so a 3xx is an error too.
async fn fetch_text(url: &str) -> Result<String, String> {
	let response = CLIENT.get(url).send().await.map_err(|e| format!("Failed to request {url}: {e}"))?;
	if !response.status().is_success() {
		return Err(format!("Failed to request {url}: HTTP {}", response.status()));
	}
	response.text().await.map_err(|e| format!("Failed to read {url}: {e}"))
}

/// Serves fetched text, or a 502 when the upstream fetch failed.
fn text_response(body: Result<String, String>, content_type: &str) -> Response<Body> {
	match body {
		Ok(body) => Response::builder().status(200).header("content-type", content_type).body(Body::from(body)),
		Err(e) => {
			warn!("{e}");
			Response::builder()
				.status(502)
				.header("content-type", "text/plain")
				.body(Body::from("Upstream request failed"))
		}
	}
	.unwrap_or_default()
}

#[cfg(test)]
mod tests {
	use super::*;
	use sealed_test::prelude::*;

	#[tokio::test(flavor = "multi_thread")]
	async fn unreachable_update_source_becomes_bad_gateway() {
		// Bind and immediately release a port so nothing is listening on it.
		let address = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap();
		let fetched = fetch_text(&format!("http://{address}/commits.atom")).await;
		assert!(fetched.is_err());
		assert_eq!(text_response(fetched, "application/atom+xml").status(), 502);
	}

	fn listener_for(args: &[&str]) -> Result<SocketAddr, String> {
		let matches = cli()
			.try_get_matches_from(std::iter::once("redlib").chain(args.iter().copied()))
			.map_err(|e| e.to_string())?;
		listener_address(&matches)
	}

	#[test]
	#[sealed_test(env = [("IPV4_ONLY", "false")])]
	fn startup_config_and_bind_false_env_does_not_enable_ipv4_only() {
		assert_eq!(listener_for(&[]).unwrap(), "[::]:8080".parse().unwrap());
	}

	#[test]
	#[sealed_test(env = [("IPV4_ONLY", "true")])]
	fn startup_config_and_bind_true_env_enables_ipv4_only() {
		assert_eq!(listener_for(&[]).unwrap(), "0.0.0.0:8080".parse().unwrap());
	}

	#[test]
	#[sealed_test(env = [("IPV4_ONLY", ""), ("IPV6_ONLY", "")])]
	fn startup_config_and_bind_empty_env_value_counts_as_unset() {
		assert_eq!(listener_for(&[]).unwrap(), "[::]:8080".parse().unwrap());
	}

	#[test]
	#[sealed_test(env = [("IPV6_ONLY", "maybe")])]
	fn startup_config_and_bind_invalid_env_value_is_rejected() {
		assert!(listener_for(&[]).is_err());
	}

	#[test]
	#[sealed_test(env = [("IPV4_ONLY", "1")])]
	fn startup_config_and_bind_explicit_address_is_kept_with_family_flag() {
		assert_eq!(listener_for(&["-a", "127.0.0.1"]).unwrap(), "127.0.0.1:8080".parse().unwrap());
		assert_eq!(listener_for(&["-4", "-a", "192.168.1.5", "-p", "9000"]).unwrap(), "192.168.1.5:9000".parse().unwrap());
	}

	#[test]
	fn startup_config_and_bind_family_flags_pick_the_wildcard_address() {
		assert_eq!(listener_for(&[]).unwrap(), "[::]:8080".parse().unwrap());
		assert_eq!(listener_for(&["-4"]).unwrap(), "0.0.0.0:8080".parse().unwrap());
		assert_eq!(listener_for(&["-6"]).unwrap(), "[::]:8080".parse().unwrap());
		assert_eq!(listener_for(&["-6", "-a", "[::1]"]).unwrap(), "[::1]:8080".parse().unwrap());
		assert_eq!(listener_for(&["-a", "::1"]).unwrap(), "[::1]:8080".parse().unwrap());
	}

	#[test]
	fn startup_config_and_bind_conflicting_flags_are_errors() {
		assert!(listener_for(&["-4", "-6"]).is_err());
		assert!(listener_for(&["-4", "-a", "::1"]).is_err());
		assert!(listener_for(&["-6", "-a", "127.0.0.1"]).is_err());
	}

	#[test]
	#[sealed_test(env = [("IPV4_ONLY", "on"), ("IPV6_ONLY", "on")])]
	fn startup_config_and_bind_conflicting_env_flags_are_errors() {
		assert!(listener_for(&[]).unwrap_err().contains("cannot be used together"));
	}

	#[test]
	fn startup_config_and_bind_invalid_address_or_port_is_an_error_not_a_panic() {
		assert!(listener_for(&["-a", "not-an-address"]).unwrap_err().contains("not-an-address"));
		assert!(listener_for(&["-p", "http"]).unwrap_err().contains("http"));
		assert!(listener_for(&["-p", "70000"]).is_err());
	}

	fn request_with_params(path: &str, params: &[(&str, &str)]) -> Request<Body> {
		let mut request = Request::get(path).body(Body::empty()).unwrap();
		let mut route_params = route_recognizer::Params::new();
		for (name, value) in params {
			route_params.insert(name.to_string(), value.to_string());
		}
		request.set_params(route_params);
		request
	}

	async fn page_text(response: Response<Body>) -> (u16, String) {
		let status = response.status().as_u16();
		let body = hyper::body::to_bytes(response.into_body()).await.unwrap();
		(status, String::from_utf8_lossy(&body).into_owned())
	}

	// Each of these would otherwise reach `canonical_path`, which asks Reddit; the handler must answer locally.
	#[tokio::test]
	async fn dos_and_shortlink_limits_non_base36_short_links_make_no_upstream_call() {
		for id in ["Login", "wp-adm", "ab.cde", "abc-de", "ab_cde", "%2e%2e%2e"] {
			let (status, body) = page_text(short_link(request_with_params(&format!("/{id}"), &[("id", id)])).await.unwrap()).await;
			assert_eq!(status, 404, "{id}");
			assert!(body.contains("Nothing here"), "{id}: {body}");
		}
	}

	#[tokio::test]
	async fn dos_and_shortlink_limits_malformed_share_ids_make_no_upstream_call() {
		for id in ["abcdefg-hi", "abc.defghi", "abc%20defgh", "abcdefg_hij"] {
			let (status, body) = page_text(share_link(request_with_params(&format!("/r/rust/s/{id}"), &[("sub", "rust"), ("id", id)])).await.unwrap()).await;
			assert_eq!(status, 404, "{id}");
			assert!(body.contains("Nothing here"), "{id}: {body}");
		}
	}

	#[test]
	fn dos_and_shortlink_limits_id_alphabets() {
		// Real short ids are lowercase base-36; lowercase words such as `login` are valid ids and are deliberately not special-cased.
		assert!(is_short_link_id("1abc23") && is_short_link_id("zzzzz") && is_short_link_id("login"));
		assert!(!is_short_link_id("1ABC23") && !is_short_link_id("abcd") && !is_short_link_id("abcdefgh") && !is_short_link_id("ab-cde"));
		// Real share ids are mixed-case.
		assert!(is_share_link_id("Sx3Yb7LbhH") && is_share_link_id("abcdefgh"));
		assert!(!is_share_link_id("abcdefg") && !is_share_link_id("abcdefghijkl") && !is_share_link_id("abcdefg-hi"));
	}

	#[test]
	fn fetched_text_is_served_with_its_content_type() {
		let response = text_response(Ok("<feed/>".to_string()), "application/atom+xml");
		assert_eq!(response.status(), 200);
		assert_eq!(response.headers()["content-type"], "application/atom+xml");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn redirected_update_source_is_an_error() {
		use tokio::io::{AsyncReadExt, AsyncWriteExt};

		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let address = listener.local_addr().unwrap();
		let upstream = tokio::spawn(async move {
			let (mut socket, _) = listener.accept().await.unwrap();
			let mut request = Vec::new();
			while !request.ends_with(b"\r\n\r\n") {
				let mut byte = [0; 1];
				socket.read_exact(&mut byte).await.unwrap();
				request.push(byte[0]);
			}
			// GitHub answers a renamed repository with a redirect and an empty body.
			socket
				.write_all(b"HTTP/1.1 301 Moved Permanently\r\nLocation: /elsewhere\r\nContent-Length: 0\r\n\r\n")
				.await
				.unwrap();
		});

		let fetched = fetch_text(&format!("http://{address}/commits.atom")).await;
		upstream.abort();
		assert!(fetched.is_err(), "an empty redirect body must not be served and cached as the feed");
	}
}
