#![allow(clippy::cmp_owned)]

use std::collections::HashMap;

// CRATES
use crate::server::ResponseExt;
use crate::subreddit::join_until_size_limit;
use crate::utils::{read_body_limited, redirect, template, Preferences};
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

	let mut response = redirect("/settings");

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
			let value = value.replace("%26", "&").replace("%23", "#");
			if value.starts_with('/') {
				value
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
	let body = read_body_limited(req.into_body(), ENCODED_RESTORE_MAX_BYTES).await?;

	let encoded_prefs = form_urlencoded::parse(&body)
		.find(|(key, _)| key == "encoded_prefs")
		.map(|(_, value)| value)
		.ok_or_else(|| "encoded_prefs parameter not found in request body".to_string())?;

	let prefs = Preferences::from_export_string(&encoded_prefs, ENCODED_RESTORE_MAX_DECOMPRESSED_BYTES)?;

	let url = format!("/settings/restore/?{}", prefs.to_urlencoded()?);

	Ok(redirect(&url))
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
		}
		.render()
		.unwrap()
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
		let result = encoded_restore(Request::post("/settings/encoded-restore").body(body).unwrap()).await;
		assert!(result.is_err());
		assert!(
			pulled.load(Ordering::SeqCst) <= 65,
			"the reader must stop near the cap, but pulled {} chunks",
			pulled.load(Ordering::SeqCst)
		);
	}

	#[tokio::test]
	async fn dos_and_shortlink_limits_encoded_restore_rejects_a_deflate_bomb() {
		let bomb = crate::utils::deflate_compress(vec![0; 2 * 1024 * 1024]).unwrap();
		let error = encoded_restore(encoded_prefs_request(&base2048::encode(&bomb))).await.unwrap_err();
		assert!(error.contains("exceeds"), "{error}");
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
}
