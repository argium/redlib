#![allow(clippy::cmp_owned)]

use crate::utils::{
	catch_random, error, filter_posts, format_num, format_url, gating_metadata, get_filters, html_page, info, nsfw_landing, param, redirect, rewrite_urls, setting, sfw_only,
	template, to_absolute_url, val, Post, Preferences, Subreddit,
};
use crate::{client::json, server::RequestExt, server::ResponseExt};
use crate::{config, utils};
use askama::Template;
use cookie::Cookie;
use hyper::{Body, Request, Response};

use chrono::DateTime;
use regex::Regex;
use rss::{ChannelBuilder, Enclosure, Item};
use std::sync::LazyLock;
use time::{Duration, OffsetDateTime};

// STRUCTS
#[derive(Template)]
#[template(path = "subreddit.html")]
struct SubredditTemplate {
	sub: Subreddit,
	posts: Vec<Post>,
	sort: (String, String),
	ends: (String, String),
	prefs: Preferences,
	url: String,
	redirect_url: String,
	/// Whether the subreddit itself is filtered.
	is_filtered: bool,
	/// Whether all fetched posts are filtered (to differentiate between no posts fetched in the first place,
	/// and all fetched posts being filtered).
	all_posts_filtered: bool,
	/// Whether all posts were hidden because they are NSFW (and user has disabled show NSFW)
	all_posts_hidden_nsfw: bool,
	no_posts: bool,
}

#[derive(Template)]
#[template(path = "wiki.html")]
struct WikiTemplate {
	sub: String,
	wiki: String,
	page: String,
	prefs: Preferences,
	url: String,
}

#[derive(Template)]
#[template(path = "wall.html")]
struct WallTemplate {
	title: String,
	sub: String,
	msg: String,
	prefs: Preferences,
	url: String,
}

static GEO_FILTER_MATCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"geo_filter=(?<region>\w+)").unwrap());

// SERVICES
pub async fn community(req: Request<Body>) -> Result<Response<Body>, String> {
	// Build Reddit API path
	let root = req.uri().path() == "/";
	let query = req.uri().query().unwrap_or_default().to_string();
	let subscribed = setting(&req, "subscriptions");
	let front_page = setting(&req, "front_page");
	let remove_default_feeds = setting(&req, "remove_default_feeds") == "on";
	let post_sort = req.cookie("post_sort").map_or_else(|| "hot".to_string(), |c| c.value().to_string());
	let sort = req.param("sort").unwrap_or_else(|| req.param("id").unwrap_or(post_sort));

	let sub_name = req.param("sub").unwrap_or(if front_page == "default" || front_page.is_empty() {
		if subscribed.is_empty() {
			"popular".to_string()
		} else {
			subscribed.clone()
		}
	} else {
		front_page.clone()
	});

	if (sub_name == "popular" || sub_name == "all") && remove_default_feeds {
		if subscribed.is_empty() {
			return info(req, "Subscribe to some subreddits! (Default feeds disabled in settings)").await;
		} else {
			// If there are subscribed subs, but we get here, then the problem is that front_page pref is set to something besides default.
			// Tell user to go to settings and change front page to default.
			return info(
				req,
				"You have subscribed to some subreddits, but your front page is not set to default. Visit settings and change front page to default.",
			)
			.await;
		}
	}

	let quarantined = can_access_quarantine(&req, &sub_name) || root;

	// Handle random subreddits
	if let Ok(random) = catch_random(&sub_name, "").await {
		return Ok(random);
	}

	if req.param("sub").is_some() && sub_name.starts_with("u_") {
		return Ok(redirect(&["/user/", &sub_name[2..]].concat()));
	}

	// Request subreddit metadata
	let metadata = if !sub_name.contains('+') && sub_name != subscribed && sub_name != "popular" && sub_name != "all" {
		// Regular subreddit
		subreddit(&sub_name, quarantined).await
	} else if sub_name == subscribed {
		// Subscription feed
		if req.uri().path().starts_with("/r/") {
			subreddit(&sub_name, quarantined).await
		} else {
			Ok(Subreddit::default())
		}
	} else {
		// Multireddit, all, popular
		Ok(Subreddit {
			name: sub_name.clone(),
			..Subreddit::default()
		})
	};
	let sub = match gating_metadata(metadata, sfw_only()) {
		Ok(sub) => sub,
		Err(msg) => return upstream_error_page(req, sub_name, &msg).await,
	};

	let req_url = req.uri().to_string();
	// Return landing page if this post if this is NSFW community but the user
	// has disabled the display of NSFW content or if the instance is SFW-only.
	if sub.nsfw && crate::utils::should_be_nsfw_gated(&req, &req_url) {
		return Ok(nsfw_landing(req, req_url).await.unwrap_or_default());
	}

	let mut params = String::from("&raw_json=1");
	if sub_name == "popular" {
		let geo_filter = match GEO_FILTER_MATCH.captures(&query) {
			Some(geo_filter) => geo_filter["region"].to_string(),
			None => "GLOBAL".to_owned(),
		};
		params.push_str(&format!("&geo_filter={geo_filter}"));
	}

	let path = format!("/r/{}/{sort}.json?{}{params}", sub_name.replace('+', "%2B"), req.uri().query().unwrap_or_default());
	let url = String::from(req.uri().path_and_query().map_or("", |val| val.as_str()));
	let redirect_url = url[1..].to_string();
	let filters = get_filters(&req);

	// If all requested subs are filtered, we don't need to fetch posts.
	if sub_name.split('+').all(|s| filters.contains(s)) {
		Ok(template(&SubredditTemplate {
			sub,
			posts: Vec::new(),
			sort: (sort, param(&path, "t").unwrap_or_default()),
			ends: (param(&path, "after").unwrap_or_default(), String::new()),
			prefs: Preferences::new(&req),
			url,
			redirect_url,
			is_filtered: true,
			all_posts_filtered: false,
			all_posts_hidden_nsfw: false,
			no_posts: false,
		}))
	} else {
		match Post::fetch(&path, quarantined).await {
			Ok((mut posts, after)) => {
				let (_, all_posts_filtered) = filter_posts(&mut posts, &filters);
				let no_posts = posts.is_empty();
				let all_posts_hidden_nsfw = !no_posts && (posts.iter().all(|p| p.flags.nsfw) && setting(&req, "show_nsfw") != "on");
				if sort == "new" {
					posts.sort_by_key(|post| std::cmp::Reverse(post.created_ts));
					posts.sort_by_key(|post| std::cmp::Reverse(post.flags.stickied));
				}
				Ok(template(&SubredditTemplate {
					sub,
					posts,
					sort: (sort, param(&path, "t").unwrap_or_default()),
					ends: (param(&path, "after").unwrap_or_default(), after),
					prefs: Preferences::new(&req),
					url,
					redirect_url,
					is_filtered: false,
					all_posts_filtered,
					all_posts_hidden_nsfw,
					no_posts,
				}))
			}
			Err(msg) => upstream_error_page(req, sub_name, &msg).await,
		}
	}
}

/// Renders the page for a failed subreddit lookup or listing fetch.
async fn upstream_error_page(req: Request<Body>, sub_name: String, msg: &str) -> Result<Response<Body>, String> {
	match msg {
		"quarantined" | "gated" => Ok(quarantine(&req, sub_name, msg)),
		"private" => error(req, &format!("r/{sub_name} is a private community")).await,
		"banned" => error(req, &format!("r/{sub_name} has been banned from Reddit")).await,
		_ => error(req, msg).await,
	}
}

pub fn quarantine(req: &Request<Body>, sub: String, restriction: &str) -> Response<Body> {
	let wall = WallTemplate {
		title: format!("r/{sub} is {restriction}"),
		msg: "Please click the button below to continue to this subreddit.".to_string(),
		url: req.uri().to_string(),
		sub,
		prefs: Preferences::new(req),
	};

	html_page(wall.render(), 403)
}

pub async fn add_quarantine_exception(req: Request<Body>) -> Result<Response<Body>, String> {
	let subreddit = req.param("sub").ok_or("Invalid URL")?;
	let redir = param(&format!("?{}", req.uri().query().unwrap_or_default()), "redir").ok_or("Invalid URL")?;
	let mut response = redirect(&redir);
	response.insert_cookie(
		Cookie::build((&format!("allow_quaran_{}", subreddit.to_lowercase()), "true"))
			.path("/")
			.http_only(true)
			.expires(cookie::Expiration::Session)
			.into(),
	);
	Ok(response)
}

pub fn can_access_quarantine(req: &Request<Body>, sub: &str) -> bool {
	// Determine if the subreddit can be accessed
	setting(req, &format!("allow_quaran_{}", sub.to_lowercase())).parse().unwrap_or_default()
}

// Join items in chunks of 4000 bytes in length for cookies
pub fn join_until_size_limit<T: std::fmt::Display>(vec: &[T]) -> Vec<std::string::String> {
	let mut result = Vec::new();
	let mut list = String::new();
	let mut current_size = 0;

	for item in vec {
		// Size in bytes
		let item_size = item.to_string().len();
		// Use 4000 bytes to leave us some headroom because the name and options of the cookie count towards the 4096 byte cap
		if current_size + item_size > 4000 {
			// If last item add a seperator on the end of the list so it's interpreted properly in tanden with the next cookie
			list.push('+');

			// Push current list to result vector
			result.push(list);

			// Reset the list variable so we can continue with only new items
			list = String::new();
		}
		// Add separator if not the first item
		if !list.is_empty() {
			list.push('+');
		}
		// Add current item to list
		list.push_str(&item.to_string());
		current_size = list.len() + item_size;
	}
	// Make sure to push whatever the remaining subreddits are there into the result vector
	result.push(list);

	// Return resulting vector
	result
}

/// Where to send the visitor after a subscribe/filter action: the optional
/// `redirect` query parameter, or the subreddit itself.
fn subscription_redirect_path(query: &str, sub: &str) -> String {
	match param(&format!("?{query}"), "redirect") {
		Some(redirect_path) => format!("/{redirect_path}"),
		None => format!("/r/{sub}"),
	}
}

/// The capitalisation Reddit uses for subreddit `name`, from its `about.json`. Falls back to `name`
/// when the lookup fails; `u_` user profiles keep their name. `fetch` is `client::json` in production.
async fn display_name<Fut>(name: &str, fetch: impl Fn(String) -> Fut) -> String
where
	Fut: std::future::Future<Output = Result<serde_json::Value, String>>,
{
	if name.starts_with("u_") {
		return name.to_string();
	}
	match fetch(format!("/r/{name}/about.json?raw_json=1")).await {
		Ok(about) => about["data"]["display_name"].as_str().unwrap_or(name).to_string(),
		Err(_) => name.to_string(),
	}
}

// Sub, filter, unfilter, or unsub by setting subscription cookie using response "Set-Cookie" header
pub async fn subscriptions_filters(req: Request<Body>) -> Result<Response<Body>, String> {
	let sub = req.param("sub").unwrap_or_default();
	let action: Vec<String> = req.uri().path().split('/').map(String::from).collect();

	// Handle random subreddits
	if sub == "random" || sub == "randnsfw" {
		if action.contains(&"filter".to_string()) || action.contains(&"unfilter".to_string()) {
			return Err("Can't filter random subreddit!".to_string());
		}
		return Err("Can't subscribe to random subreddit!".to_string());
	}

	let query = req.uri().query().unwrap_or_default().to_string();

	let preferences = Preferences::new(&req);
	let mut sub_list = preferences.subscriptions;
	let mut filters = preferences.filters;

	// Find each subreddit name (separated by '+') in sub parameter
	for part in sub.split('+').filter(|x| x != &"") {
		let part = &display_name(part, |path| json(path, true)).await;

		// Modify sub list based on action
		if action.contains(&"subscribe".to_string()) && !sub_list.contains(&part.to_owned()) {
			// Add each sub name to the subscribed list
			sub_list.push(part.to_owned());
			filters.retain(|s| s.to_lowercase() != part.to_lowercase());
			// Reorder sub names alphabetically
			sub_list.sort_by_key(|a| a.to_lowercase());
			filters.sort_by_key(|a| a.to_lowercase());
		} else if action.contains(&"unsubscribe".to_string()) {
			// Remove sub name from subscribed list
			sub_list.retain(|s| s.to_lowercase() != part.to_lowercase());
		} else if action.contains(&"filter".to_string()) && !filters.contains(&part.to_owned()) {
			// Add each sub name to the filtered list
			filters.push(part.to_owned());
			sub_list.retain(|s| s.to_lowercase() != part.to_lowercase());
			// Reorder sub names alphabetically
			filters.sort_by_key(|a| a.to_lowercase());
			sub_list.sort_by_key(|a| a.to_lowercase());
		} else if action.contains(&"unfilter".to_string()) {
			// Remove sub name from filtered list
			filters.retain(|s| s.to_lowercase() != part.to_lowercase());
		}
	}

	// Redirect back to subreddit
	// check for redirect parameter if unsubscribing/unfiltering from outside sidebar
	let mut response = redirect(&subscription_redirect_path(&query, &sub));

	// If sub_list is empty remove all subscriptions cookies, otherwise update them and remove old ones
	if sub_list.is_empty() {
		// Remove subscriptions cookie
		response.remove_cookie("subscriptions".to_string());

		// Start with first numbered subscriptions cookie
		let mut subscriptions_number = 1;

		// While whatever subscriptionsNUMBER cookie we're looking at has a value
		while req.cookie(&format!("subscriptions{subscriptions_number}")).is_some() {
			// Remove that subscriptions cookie
			response.remove_cookie(format!("subscriptions{subscriptions_number}"));

			// Increment subscriptions cookie number
			subscriptions_number += 1;
		}
	} else {
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

		// While whatever subscriptionsNUMBER cookie we're looking at has a value
		while req.cookie(&format!("subscriptions{subscriptions_number_to_delete_from}")).is_some() {
			// Remove that subscriptions cookie
			response.remove_cookie(format!("subscriptions{subscriptions_number_to_delete_from}"));

			// Increment subscriptions cookie number
			subscriptions_number_to_delete_from += 1;
		}
	}

	// If filters is empty remove all filters cookies, otherwise update them and remove old ones
	if filters.is_empty() {
		// Remove filters cookie
		response.remove_cookie("filters".to_string());

		// Start with first numbered filters cookie
		let mut filters_number = 1;

		// While whatever filtersNUMBER cookie we're looking at has a value
		while req.cookie(&format!("filters{filters_number}")).is_some() {
			// Remove that filters cookie
			response.remove_cookie(format!("filters{filters_number}"));

			// Increment filters cookie number
			filters_number += 1;
		}
	} else {
		// Start at 0 to keep track of what number we need to start deleting old filters cookies from
		let mut filters_number_to_delete_from = 0;

		for (filters_number, list) in join_until_size_limit(&filters).into_iter().enumerate() {
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

		// While whatever filtersNUMBER cookie we're looking at has a value
		while req.cookie(&format!("filters{filters_number_to_delete_from}")).is_some() {
			// Remove that filters cookie
			response.remove_cookie(format!("filters{filters_number_to_delete_from}"));

			// Increment filters cookie number
			filters_number_to_delete_from += 1;
		}
	}

	Ok(response)
}

pub async fn wiki(req: Request<Body>) -> Result<Response<Body>, String> {
	let sub = req.param("sub").unwrap_or_else(|| "reddit.com".to_string());
	let quarantined = can_access_quarantine(&req, &sub);
	// Handle random subreddits
	if let Ok(random) = catch_random(&sub, "/wiki").await {
		return Ok(random);
	}

	let page = req.param("page").unwrap_or_else(|| "index".to_string());
	let path: String = format!("/r/{sub}/wiki/{page}.json?raw_json=1");
	let url = req.uri().to_string();

	match json(path, quarantined).await {
		Ok(response) => Ok(template(&WikiTemplate {
			sub,
			wiki: rewrite_urls(response["data"]["content_html"].as_str().unwrap_or("<h3>Wiki not found</h3>")),
			page,
			prefs: Preferences::new(&req),
			url,
		})),
		Err(msg) => {
			if msg == "quarantined" || msg == "gated" {
				Ok(quarantine(&req, sub, &msg))
			} else {
				error(req, &msg).await
			}
		}
	}
}

pub async fn sidebar(req: Request<Body>) -> Result<Response<Body>, String> {
	let sub = req.param("sub").unwrap_or_else(|| "reddit.com".to_string());
	let quarantined = can_access_quarantine(&req, &sub);

	// Handle random subreddits
	if let Ok(random) = catch_random(&sub, "/about/sidebar").await {
		return Ok(random);
	}

	// Build the Reddit JSON API url
	let path: String = format!("/r/{sub}/about.json?raw_json=1");
	let url = req.uri().to_string();

	// Send a request to the url
	match json(path, quarantined).await {
		// If success, receive JSON in response
		Ok(response) => Ok(template(&WikiTemplate {
			wiki: rewrite_urls(&val(&response, "description_html")),
			// wiki: format!(
			// 	"{}<hr><h1>Moderators</h1><br><ul>{}</ul>",
			// 	rewrite_urls(&val(&response, "description_html"),
			// 	moderators(&sub, quarantined).await.unwrap_or(vec!["Could not fetch moderators".to_string()]).join(""),
			// ),
			sub,
			page: "Sidebar".to_string(),
			prefs: Preferences::new(&req),
			url,
		})),
		Err(msg) => {
			if msg == "quarantined" || msg == "gated" {
				Ok(quarantine(&req, sub, &msg))
			} else {
				error(req, &msg).await
			}
		}
	}
}

// pub async fn moderators(sub: &str, quarantined: bool) -> Result<Vec<String>, String> {
// 	// Retrieve and format the html for the moderators list
// 	Ok(
// 		moderators_list(sub, quarantined)
// 			.await?
// 			.iter()
// 			.map(|m| format!("<li><a style=\"color: var(--accent)\" href=\"/u/{name}\">{name}</a></li>", name = m))
// 			.collect(),
// 	)
// }

// async fn moderators_list(sub: &str, quarantined: bool) -> Result<Vec<String>, String> {
// 	// Build the moderator list URL
// 	let path: String = format!("/r/{}/about/moderators.json?raw_json=1", sub);

// 	// Retrieve response
// 	json(path, quarantined).await.map(|response| {
// 		// Traverse json tree and format into list of strings
// 		response["data"]["children"]
// 			.as_array()
// 			.unwrap_or(&Vec::new())
// 			.iter()
// 			.filter_map(|moderator| {
// 				let name = moderator["name"].as_str().unwrap_or_default();
// 				if name.is_empty() {
// 					None
// 				} else {
// 					Some(name.to_string())
// 				}
// 			})
// 			.collect::<Vec<_>>()
// 	})
// }

// SUBREDDIT
async fn subreddit(sub: &str, quarantined: bool) -> Result<Subreddit, String> {
	// Build the Reddit JSON API url
	let path: String = format!("/r/{sub}/about.json?raw_json=1");

	// Send a request to the url
	let res = json(path, quarantined).await?;

	// Metadata regarding the subreddit
	let members: i64 = res["data"]["subscribers"].as_u64().unwrap_or_default() as i64;
	let active: i64 = res["data"]["accounts_active"].as_u64().unwrap_or_default() as i64;

	// Fetch subreddit icon either from the community_icon or icon_img value
	let community_icon: &str = res["data"]["community_icon"].as_str().unwrap_or_default();
	let icon = if community_icon.is_empty() { val(&res, "icon_img") } else { community_icon.to_string() };

	Ok(Subreddit {
		name: val(&res, "display_name"),
		title: val(&res, "title"),
		description: val(&res, "public_description"),
		info: rewrite_urls(&val(&res, "description_html")),
		// moderators: moderators_list(sub, quarantined).await.unwrap_or_default(),
		icon: format_url(&icon),
		members: format_num(members),
		active: format_num(active),
		wiki: res["data"]["wiki_enabled"].as_bool().unwrap_or_default(),
		nsfw: res["data"]["over18"].as_bool().unwrap_or_default(),
	})
}

pub async fn rss(req: Request<Body>) -> Result<Response<Body>, String> {
	if !utils::enable_rss() {
		return Ok(error(req, "RSS is disabled on this instance.").await.unwrap_or_default());
	}

	use hyper::header::CONTENT_TYPE;

	// Get subreddit
	let sub = req.param("sub").unwrap_or_default();
	let query = format!("?{}", req.uri().query().unwrap_or_default());
	let sort = match param(&query, "sort").as_deref() {
		Some(s @ ("hot" | "new" | "top" | "rising" | "controversial")) => s.to_string(),
		_ => "hot".to_string(),
	};

	// Get path
	let path = feed_path(&sub, &sort, req.uri().query().unwrap_or_default());

	// Get subreddit link
	let subreddit_link: String = format!("{}/r/{sub}", config::get_setting("REDLIB_FULL_URL").unwrap_or_default());

	// Get subreddit data
	let subreddit = subreddit(&sub, false).await?;
	if subreddit.nsfw && sfw_only() {
		return Ok(error(req, "RSS is unavailable for NSFW communities on this instance.").await.unwrap_or_default());
	}

	// Get posts
	let (posts, _) = Post::fetch(&path, false).await?;

	let body = feed_xml(&subreddit, &subreddit_link, posts).into_bytes();

	// Create the HTTP response
	let mut res = Response::new(Body::from(body));
	res.headers_mut().insert(CONTENT_TYPE, hyper::header::HeaderValue::from_static("application/rss+xml"));

	Ok(res)
}

/// The listing a subreddit feed is built from. `raw_json=1` stops Reddit entity-escaping titles,
/// which the RSS writer escapes itself.
fn feed_path(sub: &str, sort: &str, query: &str) -> String {
	format!("/r/{sub}/{sort}.json?{query}&raw_json=1")
}

/// The RSS 2.0 document for a subreddit's posts.
fn feed_xml(subreddit: &Subreddit, link: &str, posts: Vec<Post>) -> String {
	ChannelBuilder::default()
		.title(&subreddit.title)
		.description(&subreddit.description)
		.link(link)
		.items(
			posts
				.into_iter()
				.map(|post| {
					let mut item = Item {
						title: Some(post.title.to_string()),
						link: Some(format_url(&utils::get_post_url(&post))),
						author: Some(post.author.name.to_string()),
						content: Some(rewrite_urls(&post.body)),
						pub_date: Some(DateTime::from_timestamp(post.created_ts as i64, 0).unwrap_or_default().to_rfc2822()),
						description: Some(format!("<a href='{}'>Comments</a>", to_absolute_url(&post.permalink))),
						..Default::default()
					};

					apply_enclosure(&mut item, &post);
					item
				})
				.collect::<Vec<_>>(),
		)
		.build()
		.to_string()
}

// Set enclosure image for RSS feed item
fn apply_enclosure(item: &mut Item, post: &Post) {
	item.set_enclosure(get_rss_image(post));

	// Embed the number of gallery images in description and content since
	// only the first image in the gallery is used for the enclosure
	if post.post_type == "gallery" && post.gallery.len() > 1 {
		item.set_description(format!("<a href='{}'>Gallery with {} images</a>", to_absolute_url(&post.permalink), post.gallery.len()));

		if let Some(content) = item.content() {
			let new_content = format!("{}<br/>{}", item.description().unwrap_or(""), content,);
			item.set_content(new_content);
		}
	}
}

fn get_rss_image(post: &Post) -> Option<Enclosure> {
	let image_url = match post.post_type.as_str() {
		"image" => Some(post.media.url.clone()),
		"gallery" => post.gallery.first().map(|media| media.url.clone()),
		"gif" | "video" => Some(post.media.poster.clone()),
		_ => None,
	};

	image_url.map(|url| {
		let mut enclosure = Enclosure::default();
		enclosure.set_mime_type(get_mime_type(&url));
		enclosure.set_url(to_absolute_url(&url));
		enclosure.set_length("0");
		enclosure
	})
}

/// Determines the MIME type based on file extension in a URL.
/// Handles both absolute and relative URLs with query parameters.
fn get_mime_type(url: &str) -> &'static str {
	// Extract the path component, removing query parameters
	let path = url.split('?').next().unwrap_or(url);

	// Get the file extension (everything after the last dot)
	let extension = path.rsplit('.').next().unwrap_or("").to_lowercase();

	// Match common image extensions
	match extension.as_str() {
		"jpg" | "jpeg" => "image/jpeg",
		"png" => "image/png",
		"gif" => "image/gif",
		"webp" => "image/webp",
		"svg" => "image/svg+xml",
		_ => "application/octet-stream",
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::test_support::post_fixture;
	use route_recognizer::Params;
	use sealed_test::prelude::*;
	use std::sync::Mutex;

	#[tokio::test]
	async fn vary_singleflight_and_cookies_display_names_use_about_json() {
		let requested = Mutex::new(Vec::new());
		let fetch = |path: String| {
			requested.lock().unwrap().push(path);
			async { Ok(serde_json::json!({ "data": { "display_name": "Rust" } })) }
		};

		assert_eq!(display_name("rust", fetch).await, "Rust");
		assert_eq!(
			*requested.lock().unwrap(),
			["/r/rust/about.json?raw_json=1"],
			"only about.json is needed to learn the display name"
		);

		let never_called = |_: String| async { panic!("user profiles need no lookup") };
		assert_eq!(display_name("u_spez", never_called).await, "u_spez");

		let failing = |_: String| async { Err("quarantined".to_string()) };
		assert_eq!(display_name("rust", failing).await, "rust", "a failed lookup keeps the name as given");
	}

	async fn rendered_listing(use_hls: &str, hls_post: bool) -> String {
		rendered_listing_of(vec![post_fixture(hls_post).await], use_hls)
	}

	fn rendered_listing_of(posts: Vec<Post>, use_hls: &str) -> String {
		rendered_page(posts, use_hls, "/r/test")
	}

	fn rendered_page(posts: Vec<Post>, use_hls: &str, url: &str) -> String {
		SubredditTemplate {
			sub: Subreddit {
				name: "rust".to_string(),
				..Subreddit::default()
			},
			posts,
			sort: ("hot".to_string(), "".to_string()),
			ends: (String::new(), String::new()),
			prefs: Preferences {
				use_hls: use_hls.to_string(),
				..Preferences::default()
			},
			url: url.to_string(),
			redirect_url: url[1..].to_string(),
			is_filtered: false,
			all_posts_filtered: false,
			all_posts_hidden_nsfw: false,
			no_posts: false,
		}
		.render()
		.unwrap()
	}

	/// Listing pages: link, card-layout video and HLS posts, each with the sidebar on.
	async fn listing_pages() -> Vec<(&'static str, String)> {
		let video = crate::test_support::post_of_type("video", "/vid/a/720.mp4", "").await;
		vec![
			("link listing", rendered_page(vec![post_fixture(false).await], "off", "/r/test")),
			("video listing", rendered_page(vec![video], "off", "/r/test")),
			("HLS listing", rendered_page(vec![post_fixture(true).await], "on", "/r/test")),
		]
	}

	#[tokio::test]
	async fn template_a11y_and_html_listing_pages_are_accessible() {
		for (page, html) in listing_pages().await {
			crate::html_checks::assert_accessible(page, &html);
		}
	}

	/// The `redirect=` value of every link or form in `html`, decoded the way the handlers decode it.
	fn redirect_targets(html: &str) -> Vec<String> {
		html
			.split("redirect=")
			.skip(1)
			.map(|rest| {
				let value = &rest[..rest.find('"').unwrap()];
				url::form_urlencoded::parse(format!("redirect={value}").as_bytes()).next().unwrap().1.into_owned()
			})
			.collect()
	}

	#[tokio::test]
	async fn return_links_bring_the_visitor_back_to_the_exact_page() {
		let page = "/r/rust/top?t=week&after=t3_a+b%25&q=c%23";
		let html = rendered_page(vec![post_fixture(true).await], "off", page);
		let targets = redirect_targets(&html);
		assert!(targets.len() >= 3, "subscribe/filter forms and the HLS notice links: {targets:?}");
		for target in targets {
			let back = format!("/{}", target.trim_start_matches('/'));
			assert!(back == page || back.starts_with(&format!("{page}#")), "{back} should return to {page}");
		}
	}

	#[tokio::test]
	async fn static_js_caching_subreddit_loads_hls_only_for_hls_posts() {
		let with_hls = rendered_listing("on", true).await;
		let hls = format!("/hls.min.js?v={}\" defer", crate::assets::version("hls.min.js"));
		let player = format!("/playHLSVideo.js?v={}\" defer", crate::assets::version("playHLSVideo.js"));
		assert!(with_hls.contains(&hls) && with_hls.contains(&player), "content-versioned, deferred HLS scripts expected");
		assert!(with_hls.find(&hls) < with_hls.find(&player), "hls.min.js must come first so playHLSVideo.js can use Hls");
		assert!(!rendered_listing("on", false).await.contains("hls.min.js"), "no HLS post, no hls.min.js");
		assert!(!rendered_listing("off", true).await.contains("hls.min.js"), "use_hls off, no hls.min.js");
	}

	#[tokio::test]
	async fn flair_gallery_and_prefs_versioning_links_listing_flair_search_encodes_the_flair() {
		let html = rendered_listing_of(vec![crate::test_support::flaired_post().await], "off");
		assert_eq!(crate::test_support::flair_search_links(&html), [crate::test_support::AWKWARD_FLAIR_SEARCH_LINK]);
	}

	async fn body_text(response: Response<Body>) -> String {
		String::from_utf8(hyper::body::to_bytes(response.into_body()).await.unwrap().to_vec()).unwrap()
	}

	#[tokio::test]
	async fn flair_gallery_and_prefs_versioning_links_wall_form_returns_to_the_full_url() {
		let request = Request::get("/r/edgy?sort=new&t=week").body(Body::empty()).unwrap();
		let wall = body_text(quarantine(&request, "edgy".to_string(), "quarantined")).await;

		// Submitting the wall's form must send the visitor back to the page they asked for, query string included.
		let action = wall.split("action=\"").nth(1).and_then(|rest| rest.split('"').next()).expect("the wall has a form");
		let mut submit = Request::post(action).body(Body::empty()).unwrap();
		let mut params = Params::new();
		params.insert("sub".to_string(), "edgy".to_string());
		submit.set_params(params);
		assert_eq!(add_quarantine_exception(submit).await.unwrap().headers()["Location"], "/r/edgy?sort=new&t=week", "{action}");
	}

	#[test]
	fn rss_feeds_subreddit_listing_is_fetched_as_raw_json() {
		assert_eq!(feed_path("rust", "top", "t=week"), "/r/rust/top.json?t=week&raw_json=1");
	}

	#[tokio::test]
	async fn rss_feeds_subreddit_feed_keeps_text_as_the_visitor_wrote_it() {
		let subreddit = Subreddit {
			title: "Rust & Co".to_string(),
			..Subreddit::default()
		};
		let xml = feed_xml(&subreddit, "https://redlib.example/r/rust", crate::test_support::feed_posts().await);

		let channel = rss::Channel::read_from(xml.as_bytes()).expect("the feed is well-formed RSS");
		let item = &channel.items()[0];
		assert_eq!((channel.title(), channel.link()), ("Rust & Co", "https://redlib.example/r/rust"));
		assert_eq!(item.title(), Some("Q&A <b>\"x\"</b>"));
		let content = item.content().unwrap();
		assert!(content.contains("I typed &lt;img src=x&gt; &amp; left"), "{content}");
		assert!(!content.contains("<img"), "typed text must not become markup: {content}");
	}

	#[test]
	#[sealed_test(env = [("REDLIB_ENABLE_RSS", "off")])]
	fn rss_feeds_subreddit_off_refuses_before_fetching() {
		let request = Request::get("/r/rust.rss").body(Body::empty()).unwrap();
		assert!(crate::test_support::answer_without_fetching(rss, request).contains("RSS is disabled"));
	}

	#[test]
	fn redirect_safety_subscription_target() {
		let followed = |query: &str| redirect(&subscription_redirect_path(query, "rust")).headers()["Location"].to_str().unwrap().to_string();
		// `redirect=/evil.com` is formatted as `//evil.com`, which browsers treat as another host.
		assert_eq!(followed("redirect=/evil.com"), "/");
		assert_eq!(followed("redirect=//evil.com"), "/");
		assert_eq!(followed("redirect=r/rust%3Fsort%3Dnew"), "/r/rust?sort=new");
		assert_eq!(followed(""), "/r/rust");
	}

	#[tokio::test]
	async fn redirect_safety_quarantine_exception() {
		let mut req = Request::post("/r/edgy?redir=https://evil.com").body(Body::empty()).unwrap();
		let mut params = Params::new();
		params.insert("sub".to_string(), "edgy".to_string());
		req.set_params(params);

		let response = add_quarantine_exception(req).await.unwrap();
		assert_eq!(response.headers()["Location"], "/");
		assert!(response.headers()["Set-Cookie"].to_str().unwrap().starts_with("allow_quaran_edgy=true"));
	}

	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "requires live Reddit"]
	async fn test_fetching_subreddit() {
		let subreddit = subreddit("rust", false).await;
		assert!(subreddit.is_ok());
	}

	#[tokio::test(flavor = "multi_thread")]
	#[ignore = "requires live Reddit"]
	async fn test_gated_and_quarantined() {
		let quarantined = subreddit("edgy", true).await;
		assert!(quarantined.is_ok());
		let gated = subreddit("drugs", true).await;
		assert!(gated.is_ok());
	}
}
