#![allow(clippy::cmp_owned)]
use crate::client::json;
use crate::server::RequestExt;
use crate::utils::{error, filter_posts, format_url, gating_metadata, get_filters, nsfw_landing, param, setting, sfw_only, template, Post, Preferences, User};
use crate::{config, utils};
use askama::Template;
use chrono::DateTime;
use hyper::{Body, Request, Response};
use time::{macros::format_description, OffsetDateTime};

// STRUCTS
#[derive(Template)]
#[template(path = "user.html")]
struct UserTemplate {
	user: User,
	posts: Vec<Post>,
	sort: (String, String),
	ends: (String, String),
	/// "overview", "comments", or "submitted"
	listing: String,
	prefs: Preferences,
	url: String,
	redirect_url: String,
	/// Whether the user themself is filtered.
	is_filtered: bool,
	/// Whether all fetched posts are filtered (to differentiate between no posts fetched in the first place,
	/// and all fetched posts being filtered).
	all_posts_filtered: bool,
	/// Whether all posts were hidden because they are NSFW (and user has disabled show NSFW)
	all_posts_hidden_nsfw: bool,
	no_posts: bool,
}

// FUNCTIONS
pub async fn profile(req: Request<Body>) -> Result<Response<Body>, String> {
	let listing = req.param("listing").unwrap_or_else(|| "overview".to_string());

	// Build the Reddit JSON API path
	let path = format!(
		"/user/{}/{listing}.json?{}&raw_json=1",
		req.param("name").unwrap_or_else(|| "reddit".to_string()),
		req.uri().query().unwrap_or_default(),
	);
	let url = String::from(req.uri().path_and_query().map_or("", |val| val.as_str()));
	let redirect_url = url[1..].to_string();

	// Retrieve other variables from Redlib request
	let sort = param(&path, "sort").unwrap_or_default();
	let username = req.param("name").unwrap_or_default();

	// Retrieve info from user about page.
	let user = match gating_metadata(user(&username).await, sfw_only()) {
		Ok(user) => user,
		Err(msg) => return error(req, &msg).await,
	};

	let req_url = req.uri().to_string();
	// Return landing page if this post if this Reddit deems this user NSFW,
	// but we have also disabled the display of NSFW content or if the instance
	// is SFW-only.
	if user.nsfw && utils::should_be_nsfw_gated(&req, &req_url) {
		return Ok(nsfw_landing(req, req_url).await.unwrap_or_default());
	}

	let filters = get_filters(&req);
	if filters.contains(&["u_", &username].concat()) {
		Ok(template(&UserTemplate {
			user,
			posts: Vec::new(),
			sort: (sort, param(&path, "t").unwrap_or_default()),
			ends: (param(&path, "after").unwrap_or_default(), String::new()),
			listing,
			prefs: Preferences::new(&req),
			url,
			redirect_url,
			is_filtered: true,
			all_posts_filtered: false,
			all_posts_hidden_nsfw: false,
			no_posts: false,
		}))
	} else {
		// Request user posts/comments from Reddit
		match Post::fetch(&path, false).await {
			Ok((mut posts, after)) => {
				let (_, all_posts_filtered) = filter_posts(&mut posts, &filters);
				let no_posts = posts.is_empty();
				let all_posts_hidden_nsfw = !no_posts && (posts.iter().all(|p| p.flags.nsfw) && setting(&req, "show_nsfw") != "on");
				Ok(template(&UserTemplate {
					user,
					posts,
					sort: (sort, param(&path, "t").unwrap_or_default()),
					ends: (param(&path, "after").unwrap_or_default(), after),
					listing,
					prefs: Preferences::new(&req),
					url,
					redirect_url,
					is_filtered: false,
					all_posts_filtered,
					all_posts_hidden_nsfw,
					no_posts,
				}))
			}
			// If there is an error show error page
			Err(msg) => error(req, &msg).await,
		}
	}
}

// USER
async fn user(name: &str) -> Result<User, String> {
	// Build the Reddit JSON API path
	let path: String = format!("/user/{name}/about.json?raw_json=1");

	// Send a request to the url
	json(path, false).await.map(|res| {
		// Grab creation date as unix timestamp
		let created_unix = res["data"]["created"].as_f64().unwrap_or(0.0).round() as i64;
		let created = OffsetDateTime::from_unix_timestamp(created_unix).unwrap_or(OffsetDateTime::UNIX_EPOCH);

		// Closure used to parse JSON from Reddit APIs
		let about = |item| res["data"]["subreddit"][item].as_str().unwrap_or_default().to_string();

		// Parse the JSON output into a User struct
		User {
			name: res["data"]["name"].as_str().unwrap_or(name).to_owned(),
			title: about("title"),
			icon: format_url(&about("icon_img")),
			karma: res["data"]["total_karma"].as_i64().unwrap_or(0),
			created: created.format(format_description!("[month repr:short] [day] '[year repr:last_two]")).unwrap_or_default(),
			banner: about("banner_img"),
			description: about("public_description"),
			nsfw: res["data"]["subreddit"]["over_18"].as_bool().unwrap_or_default(),
		}
	})
}

pub async fn rss(req: Request<Body>) -> Result<Response<Body>, String> {
	if !utils::enable_rss() {
		return Ok(error(req, "RSS is disabled on this instance.").await.unwrap_or_default());
	}
	use hyper::header::CONTENT_TYPE;

	// Get user
	let user_str = req.param("name").unwrap_or_default();

	let listing = req.param("listing").unwrap_or_else(|| "overview".to_string());

	// Get path
	let path = format!("/user/{user_str}/{listing}.json?{}&raw_json=1", req.uri().query().unwrap_or_default(),);

	// Get user
	let user_obj = gating_metadata(user(&user_str).await, sfw_only())?;
	if user_obj.nsfw && sfw_only() {
		return Ok(error(req, "RSS is unavailable for NSFW users on this instance.").await.unwrap_or_default());
	}

	// Get posts
	let (posts, _) = Post::fetch(&path, false).await?;

	let link = format!("{}/user/{user_str}", config::get_setting("REDLIB_FULL_URL").unwrap_or_default());
	let body = feed_xml(&user_str, &user_obj.description, &link, posts).into_bytes();

	// Create the HTTP response
	let mut res = Response::new(Body::from(body));
	res.headers_mut().insert(CONTENT_TYPE, hyper::header::HeaderValue::from_static("application/rss+xml"));

	Ok(res)
}

/// The RSS 2.0 document for a user's posts and comments.
fn feed_xml(name: &str, description: &str, link: &str, posts: Vec<Post>) -> String {
	use crate::utils::rewrite_urls;
	use rss::{ChannelBuilder, Item};

	ChannelBuilder::default()
		.title(name)
		.description(description)
		.link(link)
		.items(
			posts
				.into_iter()
				.map(|post| Item {
					title: Some(post.title.to_string()),
					link: Some(format_url(&utils::get_post_url(&post))),
					author: Some(post.author.name),
					pub_date: Some(DateTime::from_timestamp(post.created_ts as i64, 0).unwrap_or_default().to_rfc2822()),
					content: Some(rewrite_urls(&post.body)),
					..Default::default()
				})
				.collect::<Vec<_>>(),
		)
		.build()
		.to_string()
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::post_fixture;
	use sealed_test::prelude::*;

	async fn rendered_profile(use_hls: &str, hls_post: bool) -> String {
		UserTemplate {
			user: User::default(),
			posts: vec![post_fixture(hls_post).await],
			sort: ("hot".to_string(), "".to_string()),
			ends: (String::new(), String::new()),
			listing: "overview".to_string(),
			prefs: Preferences {
				use_hls: use_hls.to_string(),
				..Preferences::default()
			},
			url: "/user/test".to_string(),
			redirect_url: "/user/test".to_string(),
			is_filtered: false,
			all_posts_filtered: false,
			all_posts_hidden_nsfw: false,
			no_posts: false,
		}
		.render()
		.unwrap()
	}

	#[sealed_test(env = [("REDLIB_ENABLE_RSS", "on")])]
	fn html_markup_and_highlighted_profile_rss_button_is_not_inside_a_link() {
		let html = tokio::runtime::Runtime::new().unwrap().block_on(rendered_profile("off", false));

		crate::html_checks::assert_valid_markup("profile with RSS", &html);
		assert!(html.contains("<form action=\"/u/") && html.contains(".rss\">"), "the RSS control is still rendered: {html}");
	}

	#[tokio::test]
	async fn html_markup_and_highlighted_profile_pages_have_valid_markup() {
		crate::html_checks::assert_valid_markup("profile with a link post", &rendered_profile("off", false).await);
		crate::html_checks::assert_valid_markup("profile with an HLS post", &rendered_profile("on", true).await);
	}

	#[tokio::test]
	async fn a11y_markup_profile_page_has_a_skip_link_main_and_one_h1() {
		crate::html_checks::assert_landmarks("profile", &rendered_profile("off", false).await);
	}

	#[tokio::test]
	async fn template_a11y_and_html_profile_pages_are_accessible() {
		crate::html_checks::assert_accessible("profile with a link post", &rendered_profile("off", false).await);
		crate::html_checks::assert_accessible("profile with an HLS post", &rendered_profile("on", true).await);
	}

	#[tokio::test]
	async fn static_js_caching_user_loads_hls_only_for_hls_posts() {
		let with_hls = rendered_profile("on", true).await;
		let hls = format!("/hls.min.js?v={}\" defer", crate::assets::version("hls.min.js"));
		let player = format!("/playHLSVideo.js?v={}\" defer", crate::assets::version("playHLSVideo.js"));
		assert!(with_hls.contains(&hls) && with_hls.contains(&player), "content-versioned, deferred HLS scripts expected");
		assert!(with_hls.find(&hls) < with_hls.find(&player), "hls.min.js must come first so playHLSVideo.js can use Hls");
		assert!(!rendered_profile("on", false).await.contains("hls.min.js"), "no HLS post, no hls.min.js");
		assert!(!rendered_profile("off", true).await.contains("hls.min.js"), "use_hls off, no hls.min.js");
	}

	#[tokio::test]
	async fn rss_feeds_user_feed_has_a_link_and_keeps_typed_text_as_text() {
		let xml = feed_xml("spez", "About spez", "https://redlib.example/user/spez", crate::test_support::feed_posts().await);

		let channel = rss::Channel::read_from(xml.as_bytes()).expect("the feed is well-formed RSS");
		assert_eq!(channel.link(), "https://redlib.example/user/spez");
		let item = &channel.items()[0];
		assert_eq!(item.title(), Some("Q&A <b>\"x\"</b>"));
		let content = item.content().unwrap();
		assert!(content.contains("I typed &lt;img src=x&gt; &amp; left"), "{content}");
		assert!(!content.contains("<img"), "typed text must not become markup: {content}");
	}

	#[test]
	#[sealed_test(env = [("REDLIB_ENABLE_RSS", "off")])]
	fn rss_feeds_user_off_refuses_before_fetching() {
		let request = Request::get("/user/spez.rss").body(Body::empty()).unwrap();
		assert!(crate::test_support::answer_without_fetching(rss, request).contains("RSS is disabled"));
	}

	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "requires live Reddit"]
	async fn test_fetching_user() {
		let user = user("spez").await;
		assert!(user.is_ok());
		assert!(user.unwrap().karma > 100);
	}
}
