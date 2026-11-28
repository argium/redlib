#![allow(clippy::cmp_owned)]

use std::collections::HashMap;

// CRATES
use crate::server::ResponseExt;
use crate::subreddit::join_until_size_limit;
use crate::utils::{param, read_body_limited, redirect, template, Preferences};
use askama::Template;
use cookie::Cookie;
use hyper::{Body, Request, Response};
use time::{Duration, OffsetDateTime};
use url::form_urlencoded;

// STRUCTS
#[derive(Template)]
#[template(path = "settings.html")]
struct SettingsTemplate {
	prefs: Preferences,
	url: String,
	notice: Option<SettingsNotice>,
}

/// What the settings page says after a form, picked from `?status=` so a link cannot put its own words on the page.
enum SettingsNotice {
	Saved,
	Reset,
	ImportFailed,
}

impl SettingsNotice {
	fn from_status(status: &str) -> Option<Self> {
		match status {
			"saved" => Some(Self::Saved),
			"reset" => Some(Self::Reset),
			"import_failed" => Some(Self::ImportFailed),
			_ => None,
		}
	}

	fn message(&self) -> &'static str {
		match self {
			Self::Saved => "Settings saved.",
			Self::Reset => "Settings reset to their defaults.",
			Self::ImportFailed => "That settings export could not be read. Copy the whole text from the export box and try again.",
		}
	}

	fn is_error(&self) -> bool {
		matches!(self, Self::ImportFailed)
	}
}

// CONSTANTS

/// A settings form carries at most ~20 short preference values.
const SETTINGS_FORM_MAX_BYTES: usize = 64 * 1024;
const ENCODED_RESTORE_MAX_BYTES: usize = 1024 * 1024;
const ENCODED_RESTORE_MAX_DECOMPRESSED_BYTES: u64 = 1024 * 1024;

const PREFS: [&str; 19] = [
	"theme",
	"front_page",
	"layout",
	"wide",
	"comment_sort",
	"post_sort",
	"blur_spoiler",
	"show_nsfw",
	"blur_nsfw",
	"use_hls",
	"hide_hls_notification",
	"autoplay_videos",
	"hide_sidebar_and_summary",
	"fixed_navbar",
	"hide_awards",
	"hide_score",
	"disable_visit_reddit_confirmation",
	"video_quality",
	"remove_default_feeds",
];

// FUNCTIONS

/// Retrieve cookies from request "Cookie" header
pub async fn get(req: Request<Body>) -> Result<Response<Body>, String> {
	let url = req.uri().to_string();
	Ok(template(&SettingsTemplate {
		prefs: Preferences::new(&req),
		notice: param(&url, "status").and_then(|status| SettingsNotice::from_status(&status)),
		url,
	}))
}

/// Set cookies using response "Set-Cookie" header
pub async fn set(req: Request<Body>) -> Result<Response<Body>, String> {
	// Split the body into parts
	let (parts, body) = req.into_parts();

	// Grab existing cookies
	let _cookies: Vec<Cookie<'_>> = parts
		.headers
		.get_all("Cookie")
		.iter()
		.filter_map(|header| Cookie::parse(header.to_str().unwrap_or_default()).ok())
		.collect();

	let body_bytes = read_body_limited(body, SETTINGS_FORM_MAX_BYTES).await?;

	let form = url::form_urlencoded::parse(&body_bytes).collect::<HashMap<_, _>>();

	// The reset button posts an empty form: with no preference in it, every preference cookie below is removed.
	let mut response = redirect(if form.contains_key("reset") {
		"/settings?status=reset"
	} else {
		"/settings?status=saved"
	});

	for &name in &PREFS {
		match form.get(name) {
			Some(value) => response.insert_cookie(
				Cookie::build((name.to_owned(), value.clone()))
					.path("/")
					.http_only(true)
					.expires(OffsetDateTime::now_utc() + Duration::weeks(52))
					.into(),
			),
			None => response.remove_cookie(name.to_string()),
		};
	}

	Ok(response)
}

fn set_cookies_method(req: Request<Body>, remove_cookies: bool) -> Response<Body> {
	// Split the body into parts
	let (parts, _) = req.into_parts();

	// Grab existing cookies
	let _cookies: Vec<Cookie<'_>> = parts
		.headers
		.get_all("Cookie")
		.iter()
		.filter_map(|header| Cookie::parse(header.to_str().unwrap_or_default()).ok())
		.collect();

	let query = parts.uri.query().unwrap_or_default().as_bytes();

	let form = url::form_urlencoded::parse(query).collect::<HashMap<_, _>>();

	let path = match form.get("redirect") {
		Some(value) => {
			if value.starts_with('/') {
				value.to_string()
			} else {
				format!("/{value}")
			}
		}
		None => "/".to_string(),
	};

	let mut response = redirect(&path);

	for name in PREFS {
		match form.get(name) {
			Some(value) => response.insert_cookie(
				Cookie::build((name.to_owned(), value.clone()))
					.path("/")
					.http_only(true)
					.expires(OffsetDateTime::now_utc() + Duration::weeks(52))
					.into(),
			),
			None => {
				if remove_cookies {
					response.remove_cookie(name.to_string());
				}
			}
		};
	}

	// Get subscriptions/filters to restore from query string
	let subscriptions = form.get("subscriptions");
	let filters = form.get("filters");

	// We can't search through the cookies directly like in subreddit.rs, so instead we have to make a string out of the request's headers to search through
	let cookies_string = parts
		.headers
		.get("cookie")
		.map(|hv| hv.to_str().unwrap_or("").to_string()) // Return String
		.unwrap_or_else(String::new); // Return an empty string if None

	// If there are subscriptions to restore set them and delete any old subscriptions cookies, otherwise delete them all
	if let Some(subscriptions) = subscriptions {
		let sub_list: Vec<String> = subscriptions.split('+').map(str::to_string).collect();

		// Start at 0 to keep track of what number we need to start deleting old subscription cookies from
		let mut subscriptions_number_to_delete_from = 0;

		// Starting at 0 so we handle the subscription cookie without a number first
		for (subscriptions_number, list) in join_until_size_limit(&sub_list).into_iter().enumerate() {
			let subscriptions_cookie = if subscriptions_number == 0 {
				"subscriptions".to_string()
			} else {
				format!("subscriptions{subscriptions_number}")
			};

			response.insert_cookie(
				Cookie::build((subscriptions_cookie, list))
					.path("/")
					.http_only(true)
					.expires(OffsetDateTime::now_utc() + Duration::weeks(52))
					.into(),
			);

			subscriptions_number_to_delete_from += 1;
		}

		// While subscriptionsNUMBER= is in the string of cookies add a response removing that cookie
		while cookies_string.contains(&format!("subscriptions{subscriptions_number_to_delete_from}=")) {
			// Remove that subscriptions cookie
			response.remove_cookie(format!("subscriptions{subscriptions_number_to_delete_from}"));

			// Increment subscriptions cookie number
			subscriptions_number_to_delete_from += 1;
		}
	} else if remove_cookies {
		// Remove unnumbered subscriptions cookie
		response.remove_cookie("subscriptions".to_string());

		// Starts at one to deal with the first numbered subscription cookie and onwards
		let mut subscriptions_number_to_delete_from = 1;

		// While subscriptionsNUMBER= is in the string of cookies add a response removing that cookie
		while cookies_string.contains(&format!("subscriptions{subscriptions_number_to_delete_from}=")) {
			// Remove that subscriptions cookie
			response.remove_cookie(format!("subscriptions{subscriptions_number_to_delete_from}"));

			// Increment subscriptions cookie number
			subscriptions_number_to_delete_from += 1;
		}
	}

	// If there are filters to restore set them and delete any old filters cookies, otherwise delete them all
	if let Some(filters) = filters {
		let filters_list: Vec<String> = filters.split('+').map(str::to_string).collect();

		// Start at 0 to keep track of what number we need to start deleting old subscription cookies from
		let mut filters_number_to_delete_from = 0;

		// Starting at 0 so we handle the subscription cookie without a number first
		for (filters_number, list) in join_until_size_limit(&filters_list).into_iter().enumerate() {
			let filters_cookie = if filters_number == 0 {
				"filters".to_string()
			} else {
				format!("filters{filters_number}")
			};

			response.insert_cookie(
				Cookie::build((filters_cookie, list))
					.path("/")
					.http_only(true)
					.expires(OffsetDateTime::now_utc() + Duration::weeks(52))
					.into(),
			);

			filters_number_to_delete_from += 1;
		}

		// While filtersNUMBER= is in the string of cookies add a response removing that cookie
		while cookies_string.contains(&format!("filters{filters_number_to_delete_from}=")) {
			// Remove that filters cookie
			response.remove_cookie(format!("filters{filters_number_to_delete_from}"));

			// Increment filters cookie number
			filters_number_to_delete_from += 1;
		}
	} else if remove_cookies {
		// Remove unnumbered filters cookie
		response.remove_cookie("filters".to_string());

		// Starts at one to deal with the first numbered subscription cookie and onwards
		let mut filters_number_to_delete_from = 1;

		// While filtersNUMBER= is in the string of cookies add a response removing that cookie
		while cookies_string.contains(&format!("filters{filters_number_to_delete_from}=")) {
			// Remove that sfilters cookie
			response.remove_cookie(format!("filters{filters_number_to_delete_from}"));

			// Increment filters cookie number
			filters_number_to_delete_from += 1;
		}
	}

	response
}

/// Set cookies using response "Set-Cookie" header
pub async fn restore(req: Request<Body>) -> Result<Response<Body>, String> {
	Ok(set_cookies_method(req, true))
}

pub async fn update(req: Request<Body>) -> Result<Response<Body>, String> {
	Ok(set_cookies_method(req, false))
}

pub async fn encoded_restore(req: Request<Body>) -> Result<Response<Body>, String> {
	let restore_query = read_body_limited(req.into_body(), ENCODED_RESTORE_MAX_BYTES)
		.await
		.and_then(|body| {
			form_urlencoded::parse(&body)
				.find(|(key, _)| key == "encoded_prefs")
				.map(|(_, value)| value.into_owned())
				.ok_or_else(|| "encoded_prefs parameter not found in request body".to_string())
		})
		.and_then(|encoded_prefs| Preferences::from_export_string(&encoded_prefs, ENCODED_RESTORE_MAX_DECOMPRESSED_BYTES))
		.and_then(|prefs| prefs.to_urlencoded());

	// The reason goes to the log; the visitor sees a fixed message on /settings, not the decoder's wording.
	match restore_query {
		Ok(query) => Ok(redirect(&format!("/settings/restore/?{query}"))),
		Err(error) => {
			log::warn!("Settings import refused: {error}");
			Ok(redirect("/settings?status=import_failed"))
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use futures_lite::StreamExt;
	use std::sync::{
		atomic::{AtomicUsize, Ordering},
		Arc,
	};

	/// A request body of `chunks` chunks that records how many chunks the reader pulled.
	fn counting_body(chunk_size: usize, chunks: usize) -> (Body, Arc<AtomicUsize>) {
		let pulled = Arc::new(AtomicUsize::new(0));
		let counter = pulled.clone();
		let stream = futures_lite::stream::iter(0..chunks).map(move |_| {
			counter.fetch_add(1, Ordering::SeqCst);
			Ok::<_, std::io::Error>(vec![b'a'; chunk_size])
		});
		(Body::wrap_stream(stream), pulled)
	}

	fn settings_page() -> String {
		SettingsTemplate {
			prefs: Preferences {
				available_themes: vec!["system".to_string(), "dark".to_string()],
				subscriptions: vec!["rust".to_string(), "u_spez".to_string()],
				filters: vec!["news".to_string()],
				..Preferences::default()
			},
			url: "/settings".to_string(),
			notice: None,
		}
		.render()
		.unwrap()
	}

	#[test]
	fn html_markup_and_highlighted_settings_page_has_valid_markup() {
		crate::html_checks::assert_valid_markup("settings", &settings_page());
	}

	#[test]
	fn a11y_markup_settings_page_has_a_skip_link_main_and_one_h1() {
		crate::html_checks::assert_landmarks("settings", &settings_page());
	}

	#[test]
	fn template_a11y_and_html_settings_page_is_accessible() {
		crate::html_checks::assert_accessible("settings", &settings_page());
	}

	#[test]
	fn template_a11y_and_html_settings_copy_result_is_announced_beside_the_button() {
		let html = settings_page();
		let button = html.find("id=\"copy\"").expect("the copy button");
		let status = html.find("id=\"copy_status\" role=\"status\"").expect("a status region for the copy result");
		assert!(
			status > button && html[button..status].contains("</button>"),
			"the status must sit outside and after the button"
		);
	}

	fn encoded_prefs_request(encoded_prefs: &str) -> Request<Body> {
		let body = form_urlencoded::Serializer::new(String::new()).append_pair("encoded_prefs", encoded_prefs).finish();
		Request::post("/settings/encoded-restore").body(Body::from(body)).unwrap()
	}

	#[tokio::test]
	async fn dos_and_shortlink_limits_settings_form_body_is_capped_while_streaming() {
		// 1 MiB offered in 16 KiB chunks.
		let (body, pulled) = counting_body(16 * 1024, 64);
		let result = set(Request::post("/settings").body(body).unwrap()).await;
		assert!(result.is_err(), "an oversized form must be refused");
		assert!(
			pulled.load(Ordering::SeqCst) <= 5,
			"the reader must stop near the cap, but pulled {} chunks",
			pulled.load(Ordering::SeqCst)
		);
	}

	#[tokio::test]
	async fn dos_and_shortlink_limits_settings_form_still_sets_cookies() {
		let req = Request::post("/settings").body(Body::from("theme=dark&layout=compact")).unwrap();
		let response = set(req).await.unwrap();
		let cookies: Vec<_> = response.headers().get_all("Set-Cookie").iter().map(|value| value.to_str().unwrap().to_string()).collect();
		assert!(cookies.iter().any(|cookie| cookie.starts_with("theme=dark")), "{cookies:?}");
		assert!(cookies.iter().any(|cookie| cookie.starts_with("layout=compact")), "{cookies:?}");
	}

	#[tokio::test]
	async fn dos_and_shortlink_limits_encoded_restore_body_is_capped_while_streaming() {
		// 2 MiB offered in 16 KiB chunks; the cap is 1 MiB.
		let (body, pulled) = counting_body(16 * 1024, 128);
		let response = encoded_restore(Request::post("/settings/encoded-restore").body(body).unwrap()).await.unwrap();
		assert_eq!(response.headers()["Location"], "/settings?status=import_failed");
		assert!(response.headers().get("Set-Cookie").is_none());
		assert!(
			pulled.load(Ordering::SeqCst) <= 65,
			"the reader must stop near the cap, but pulled {} chunks",
			pulled.load(Ordering::SeqCst)
		);
	}

	#[tokio::test]
	async fn settings_update_preserves_percent_encoded_query_values_in_redirect() {
		let return_uri = "/search?q=fish%26chips%23fresh";
		let encoded = percent_encoding::utf8_percent_encode(return_uri, percent_encoding::NON_ALPHANUMERIC).to_string();
		let req = Request::get(format!("/settings/update/?use_hls=on&redirect={encoded}")).body(Body::empty()).unwrap();
		let response = update(req).await.unwrap();
		assert_eq!(response.headers()["Location"], return_uri);
	}

	#[tokio::test]
	async fn dos_and_shortlink_limits_encoded_restore_rejects_a_deflate_bomb() {
		let bomb = crate::utils::deflate_compress(vec![0; 2 * 1024 * 1024]).unwrap();
		let response = encoded_restore(encoded_prefs_request(&base2048::encode(&bomb))).await.unwrap();
		assert_eq!(response.headers()["Location"], "/settings?status=import_failed");
		assert!(response.headers().get("Set-Cookie").is_none(), "a refused export must not change any setting");
	}

	#[tokio::test]
	async fn flair_gallery_and_prefs_versioning_prefs_encoded_restore_accepts_v1_and_current_exports() {
		let prefs = Preferences {
			theme: "dark".to_string(),
			..Preferences::default()
		};
		// Before versioning, an export was the deflated bincode of the preferences, as written here.
		let v1_export = base2048::encode(&crate::utils::deflate_compress(bincode::serialize(&prefs).unwrap()).unwrap());
		for encoded in [v1_export, prefs.to_export_string().unwrap()] {
			let response = encoded_restore(encoded_prefs_request(&encoded)).await.unwrap();
			let location = response.headers()["Location"].to_str().unwrap();
			assert!(location.starts_with("/settings/restore/?") && location.contains("theme=dark"), "{location}");
		}
	}

	#[tokio::test]
	async fn dos_and_shortlink_limits_encoded_restore_round_trips_preferences() {
		let encoded = Preferences::default().to_export_string().unwrap();
		let response = encoded_restore(encoded_prefs_request(&encoded)).await.unwrap();
		assert!(response.headers()["Location"].to_str().unwrap().starts_with("/settings/restore/?"));
	}

	#[tokio::test]
	async fn redirect_safety_settings_update() {
		for target in ["//evil.com", "/%5Cevil.com", "https://evil.com"] {
			let req = Request::get(format!("/settings/update?redirect={target}&theme=dark")).body(Body::empty()).unwrap();
			let location = update(req).await.unwrap().headers()["Location"].to_str().unwrap().to_string();
			// Only a same-site path may be followed; `https://evil.com` becomes the harmless path `/https://evil.com`.
			assert!(
				location.starts_with('/') && !location.starts_with("//") && !location.starts_with("/\\"),
				"{target} redirected to {location}"
			);
		}
		let req = Request::get("/settings/update?redirect=//evil.com").body(Body::empty()).unwrap();
		assert_eq!(update(req).await.unwrap().headers()["Location"], "/");
	}

	async fn settings_text(query: &str) -> String {
		let response = get(Request::get(format!("/settings{query}")).body(Body::empty()).unwrap()).await.unwrap();
		String::from_utf8(hyper::body::to_bytes(response.into_body()).await.unwrap().to_vec()).unwrap()
	}

	#[tokio::test]
	async fn ux_templates_and_settings_feedback_saving_says_so_on_the_settings_page() {
		let response = set(Request::post("/settings").body(Body::from("theme=dark")).unwrap()).await.unwrap();
		assert_eq!(response.headers()["Location"], "/settings?status=saved");

		let html = settings_text("?status=saved").await;
		assert!(html.contains("role=\"status\"") && html.contains("Settings saved."), "{html}");
	}

	#[tokio::test]
	async fn ux_templates_and_settings_feedback_reset_clears_every_preference_and_says_so() {
		let response = set(Request::post("/settings").body(Body::from("reset=1")).unwrap()).await.unwrap();
		assert_eq!(response.headers()["Location"], "/settings?status=reset");
		let cookies: Vec<_> = response.headers().get_all("Set-Cookie").iter().map(|value| value.to_str().unwrap().to_string()).collect();
		for name in PREFS {
			assert!(cookies.iter().any(|cookie| cookie.starts_with(&format!("{name}=;"))), "{name} was not cleared: {cookies:?}");
		}
		assert!(
			!cookies.iter().any(|cookie| cookie.starts_with("subscriptions")),
			"subscriptions are not preferences: {cookies:?}"
		);

		let html = settings_text("?status=reset").await;
		assert!(html.contains("role=\"status\"") && html.contains("Settings reset to their defaults."), "{html}");
	}

	#[tokio::test]
	async fn ux_templates_and_settings_feedback_a_failed_import_is_an_alert_without_internal_details() {
		let html = settings_text("?status=import_failed").await;

		assert!(html.contains("role=\"alert\"") && html.contains("could not be read"), "{html}");
		assert!(!html.contains("deflate") && !html.contains("exceeds"), "{html}");
	}

	#[tokio::test]
	async fn ux_templates_and_settings_feedback_unknown_status_shows_nothing() {
		for query in ["", "?status=%3Cb%3Ehi%3C/b%3E", "?status=Settings%20saved."] {
			let html = settings_text(query).await;
			assert!(!html.contains("settings_notice") && !html.contains("<b>hi"), "{query}: {html}");
		}
	}

	#[tokio::test]
	async fn ux_templates_and_settings_feedback_unreadable_imports_return_to_settings() {
		for request in [
			encoded_prefs_request("this is not an export"),
			Request::post("/settings/encoded-restore").body(Body::from("other=1")).unwrap(),
		] {
			let response = encoded_restore(request).await.unwrap();
			assert_eq!(response.headers()["Location"], "/settings?status=import_failed");
			assert!(response.headers().get("Set-Cookie").is_none());
		}
	}

	#[test]
	fn ux_templates_and_settings_feedback_settings_page_offers_reset_to_defaults() {
		let html = settings_page();

		let form = html.split("action=\"/settings\" method=\"POST\"").nth(2).expect("a second form posting to /settings");
		let form = form.split("</form>").next().unwrap();
		assert!(form.contains("name=\"reset\"") && form.contains("Reset to defaults"), "{form}");
	}

	#[test]
	fn themes_and_sr_labels_theme_select_shows_readable_names_and_keeps_file_names_as_values() {
		let html = SettingsTemplate {
			prefs: Preferences {
				available_themes: vec!["system".to_string(), "libredditBlack".to_string()],
				theme: "libredditBlack".to_string(),
				..Preferences::default()
			},
			url: "/settings".to_string(),
			notice: None,
		}
		.render()
		.unwrap();

		assert!(html.contains("value=\"libredditBlack\" selected>") && html.contains("Libreddit Black"), "{html}");
		assert!(!html.contains(">libredditBlack<"), "{html}");
	}
}
