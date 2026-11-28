#![allow(clippy::cmp_owned)]
use crate::client::json;
use crate::config::get_setting;
use crate::server::RequestExt;
use crate::subreddit::{can_access_quarantine, quarantine};
use crate::utils::{
	absolute_url, error, format_num, get_filters, nsfw_landing, param, parse_post, request_base_url, rewrite_emotes, setting, template, time, val, Author, Awards, Comment,
	Flair, FlairPart, Post, Preferences,
};
use askama::Template;
use hyper::{Body, Request, Response};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

// STRUCTS
#[derive(Template)]
#[template(path = "post.html")]
struct PostTemplate {
	comments: Vec<Comment>,
	post: Post,
	sort: String,
	prefs: Preferences,
	single_thread: bool,
	url: String,
	url_without_query: String,
	comment_query: String,
	/// The instance's public URL, or empty when unknown; link previews need absolute URLs.
	base_url: String,
}

impl PostTemplate {
	/// `url` as an absolute URL when the instance's public URL is known.
	fn absolute(&self, url: &str) -> String {
		absolute_url(&self.base_url, url)
	}
}

static COMMENT_SEARCH_CAPTURE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\?q=(.*)&type=comment").unwrap());

pub async fn item(req: Request<Body>) -> Result<Response<Body>, String> {
	// Build Reddit API path
	let mut path: String = format!("{}.json?{}&raw_json=1", req.uri().path(), req.uri().query().unwrap_or_default());
	let sub = req.param("sub").unwrap_or_default();
	let quarantined = can_access_quarantine(&req, &sub);
	let url = req.uri().to_string();

	// Set sort to sort query parameter
	let sort = param(&path, "sort").unwrap_or_else(|| {
		// Grab default comment sort method from Cookies
		let default_sort = setting(&req, "comment_sort");

		// If there's no sort query but there's a default sort, set sort to default_sort
		if default_sort.is_empty() {
			String::new()
		} else {
			path = format!("{}.json?{}&sort={}&raw_json=1", req.uri().path(), req.uri().query().unwrap_or_default(), default_sort);
			default_sort
		}
	});

	// Log the post ID being fetched in debug mode
	#[cfg(debug_assertions)]
	req.param("id").unwrap_or_default();

	let single_thread = req.param("comment_id").is_some();
	let highlighted_comment = &req.param("comment_id").unwrap_or_default();

	// Send a request to the url, receive JSON in response
	match json(path, quarantined).await {
		// Otherwise, grab the JSON output from the request
		Ok(response) => {
			// Parse the JSON into Post and Comment structs
			let post = parse_post(&response[0]["data"]["children"][0]).await;

			let req_url = req.uri().to_string();
			// Return landing page if this post if this Reddit deems this post
			// NSFW, but we have also disabled the display of NSFW content
			// or if the instance is SFW-only.
			if post.nsfw && crate::utils::should_be_nsfw_gated(&req, &req_url) {
				return Ok(nsfw_landing(req, req_url).await.unwrap_or_default());
			}

			let query_body = match COMMENT_SEARCH_CAPTURE.captures(&url) {
				Some(captures) => captures.get(1).unwrap().as_str().replace("%20", " ").replace('+', " "),
				None => String::new(),
			};

			let query_string = format!("q={query_body}&type=comment");
			let form = url::form_urlencoded::parse(query_string.as_bytes()).collect::<HashMap<_, _>>();
			let query = form.get("q").unwrap().clone().to_string();

			let prefs = Preferences::new(&req);
			let comments = match query.as_str() {
				"" => parse_comments(&response[1], &post.permalink, &post.author.name, highlighted_comment, &get_filters(&req), &prefs),
				_ => query_comments(&response[1], &post.permalink, &post.author.name, highlighted_comment, &get_filters(&req), &query, &prefs),
			};

			// Use the Post and Comment structs to generate a website to show users
			Ok(template(&PostTemplate {
				comments,
				post,
				url_without_query: url.clone().trim_end_matches(&format!("?q={query}&type=comment")).to_string(),
				sort,
				prefs,
				single_thread,
				url: req_url,
				comment_query: query,
				base_url: request_base_url(&req).unwrap_or_default(),
			}))
		}
		// If the Reddit API returns an error, exit and send error page to user
		Err(msg) => {
			if msg == "quarantined" || msg == "gated" {
				let sub = req.param("sub").unwrap_or_default();
				Ok(quarantine(&req, sub, &msg))
			} else {
				error(req, &msg).await
			}
		}
	}
}

// COMMENTS

fn parse_comments(json: &serde_json::Value, post_link: &str, post_author: &str, highlighted_comment: &str, filters: &HashSet<String>, prefs: &Preferences) -> Vec<Comment> {
	// Parse the comment JSON into a Vector of Comments
	let comments = json["data"]["children"].as_array().into_iter().flatten();

	// For each comment, retrieve the values to build a Comment object
	comments
		.map(|comment| {
			let data = &comment["data"];
			let replies: Vec<Comment> = if data["replies"].is_object() {
				parse_comments(&data["replies"], post_link, post_author, highlighted_comment, filters, prefs)
			} else {
				Vec::new()
			};
			build_comment(comment, data, replies, post_link, post_author, highlighted_comment, filters, prefs)
		})
		.collect()
}

fn query_comments(
	json: &serde_json::Value,
	post_link: &str,
	post_author: &str,
	highlighted_comment: &str,
	filters: &HashSet<String>,
	query: &str,
	prefs: &Preferences,
) -> Vec<Comment> {
	let comments = json["data"]["children"].as_array().into_iter().flatten();
	let mut results = Vec::new();

	for comment in comments {
		let data = &comment["data"];

		// If this comment contains replies, handle those too
		if data["replies"].is_object() {
			results.append(&mut query_comments(&data["replies"], post_link, post_author, highlighted_comment, filters, query, prefs));
		}

		// Match the comment's text: `c.body` is rendered HTML, so `href` or `class` would hit nearly every comment.
		if val(comment, "body").to_lowercase().contains(&query.to_lowercase()) {
			results.push(build_comment(comment, data, Vec::new(), post_link, post_author, highlighted_comment, filters, prefs));
		}
	}

	results
}
#[allow(clippy::too_many_arguments)]
fn build_comment(
	comment: &serde_json::Value,
	data: &serde_json::Value,
	replies: Vec<Comment>,
	post_link: &str,
	post_author: &str,
	highlighted_comment: &str,
	filters: &HashSet<String>,
	prefs: &Preferences,
) -> Comment {
	let id = val(comment, "id");

	let body = if (val(comment, "author") == "[deleted]" && val(comment, "body") == "[removed]") || val(comment, "body") == "[ Removed by Reddit ]" {
		format!(
			"<div class=\"md\"><p>[removed] — <a href=\"https://{}{post_link}{id}\">view removed comment</a></p></div>",
			get_setting("REDLIB_PUSHSHIFT_FRONTEND").unwrap_or_else(|| String::from(crate::config::DEFAULT_PUSHSHIFT_FRONTEND)),
		)
	} else {
		rewrite_emotes(&data["media_metadata"], val(comment, "body_html"))
	};
	let kind = comment["kind"].as_str().unwrap_or_default().to_string();

	let unix_time = data["created_utc"].as_f64().unwrap_or_default();
	let (rel_time, created) = time(unix_time);

	let edited = data["edited"].as_f64().map_or((String::new(), String::new()), time);

	let score = data["score"].as_i64().unwrap_or(0);

	// The JSON API only provides comments up to some threshold.
	// Further comments have to be loaded by subsequent requests.
	// The "kind" value will be "more" and the "count"
	// shows how many more (sub-)comments exist in the respective nesting level.
	// Note that in certain (seemingly random) cases, the count is simply wrong.
	let more_count = data["count"].as_i64().unwrap_or_default();

	let awards: Awards = Awards::parse(&data["all_awardings"]);

	let parent_kind_and_id = val(comment, "parent_id");
	let parent_info = parent_kind_and_id.split('_').collect::<Vec<&str>>();

	let highlighted = id == highlighted_comment;

	let author = Author {
		name: val(comment, "author"),
		flair: Flair {
			flair_parts: FlairPart::parse(
				data["author_flair_type"].as_str().unwrap_or_default(),
				data["author_flair_richtext"].as_array(),
				data["author_flair_text"].as_str(),
			),
			text: val(comment, "link_flair_text"),
			background_color: val(comment, "author_flair_background_color"),
			foreground_color: val(comment, "author_flair_text_color"),
		},
		distinguished: val(comment, "distinguished"),
	};
	let is_filtered = filters.contains(&["u_", author.name.as_str()].concat());

	// Many subreddits have a default comment posted about the sub's rules etc.
	// Many Redlib users do not wish to see this kind of comment by default.
	// Reddit does not tell us which users are "bots", so a good heuristic is to
	// collapse stickied moderator comments.
	let is_moderator_comment = data["distinguished"].as_str().unwrap_or_default() == "moderator";
	let is_stickied = data["stickied"].as_bool().unwrap_or_default();
	let collapsed = (is_moderator_comment && is_stickied) || is_filtered;

	Comment {
		id,
		kind,
		parent_id: parent_info[1].to_string(),
		parent_kind: parent_info[0].to_string(),
		post_link: post_link.to_string(),
		post_author: post_author.to_string(),
		body,
		author,
		score: if data["score_hidden"].as_bool().unwrap_or_default() {
			("\u{2022}".to_string(), "Hidden".to_string())
		} else {
			format_num(score)
		},
		rel_time,
		created,
		edited,
		replies,
		highlighted,
		awards,
		collapsed,
		is_filtered,
		more_count,
		hide_score: prefs.hide_score == "on",
		hide_awards: prefs.hide_awards == "on",
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::utils::GalleryMedia;
	use serde_json::json;

	/// A comment listing as Reddit returns it: `count` sibling comments, each with one award.
	fn comment_listing(count: usize) -> serde_json::Value {
		let children: Vec<_> = (0..count)
			.map(|index| {
				json!({
					"kind": "t1",
					"data": {
						"id": format!("c{index}"),
						"author": "someone",
						"body": "hello",
						"body_html": "<div class=\"md\"><p>hello</p></div>",
						"score": 42,
						"created_utc": 1_700_000_000.0,
						"parent_id": "t3_abc",
						"all_awardings": [{ "name": "Silver", "count": 1, "resized_icons": [{ "url": "https://preview.redd.it/silver.png" }] }],
					}
				})
			})
			.collect();
		json!({ "data": { "children": children } })
	}

	/// The visible text of a comment's score paragraph (its `title` attribute always carries the real score).
	fn score_cell(comment: &Comment) -> String {
		let html = comment.render().unwrap();
		let start = html.find("class=\"comment_score\"").expect("comment has a score cell");
		let paragraph = html[start..].split("</p>").next().unwrap();
		paragraph.split_once('>').expect("paragraph tag closes").1.trim().to_string()
	}

	/// A post page with no comments, default preferences and no known base URL.
	fn post_template(post: Post) -> PostTemplate {
		PostTemplate {
			comments: Vec::new(),
			url: post.permalink.clone(),
			url_without_query: post.permalink.clone(),
			post,
			sort: String::new(),
			prefs: Preferences::default(),
			single_thread: false,
			comment_query: String::new(),
			base_url: String::new(),
		}
	}

	fn render_post(post: Post, base_url: &str) -> String {
		PostTemplate {
			base_url: base_url.to_string(),
			..post_template(post)
		}
		.render()
		.unwrap()
	}

	/// Every shape a post page takes: thread views over each media type, a gallery with captions,
	/// and the HLS player.
	async fn post_pages() -> Vec<(&'static str, String)> {
		use crate::test_support::{post_fixture, post_of_type};
		let comments = || parse_comments(&comment_listing(2), "/r/test/comments/link1/a_link/", "op", "", &HashSet::new(), &Preferences::default());
		let gallery_item = |caption: &str, outbound_url: &str| GalleryMedia {
			url: "/preview/pre/a.jpg".to_string(),
			width: 100,
			height: 100,
			caption: caption.to_string(),
			outbound_url: outbound_url.to_string(),
		};
		let mut gallery = post_of_type("gallery", "", "").await;
		gallery.gallery = vec![gallery_item("A caption", "https://example.com/shop"), gallery_item("", "")];
		let hls_prefs = Preferences {
			use_hls: "on".to_string(),
			..Preferences::default()
		};

		let thread = |post: Post| PostTemplate {
			single_thread: true,
			comments: comments(),
			..post_template(post)
		};
		vec![
			(
				"link post with comments",
				PostTemplate {
					comments: comments(),
					..post_template(post_fixture(false).await)
				}
				.render()
				.unwrap(),
			),
			("image thread", thread(post_of_type("image", "/img/a.jpg", "/thumb/b/a.jpg").await).render().unwrap()),
			("video thread", thread(post_of_type("video", "/vid/a/720.mp4", "").await).render().unwrap()),
			(
				"gallery",
				PostTemplate {
					comments: comments(),
					..post_template(gallery)
				}
				.render()
				.unwrap(),
			),
			(
				"HLS video",
				PostTemplate {
					prefs: hls_prefs,
					..post_template(post_fixture(true).await)
				}
				.render()
				.unwrap(),
			),
		]
	}

	#[tokio::test]
	async fn html_markup_and_highlighted_post_pages_have_valid_markup() {
		for (page, html) in post_pages().await {
			crate::html_checks::assert_valid_markup(page, &html);
		}
	}

	fn highlighted_script_count(html: &str) -> usize {
		html.matches("/highlighted.js?v=").count()
	}

	#[tokio::test]
	async fn html_markup_and_highlighted_script_loads_for_every_post_type_in_a_single_thread() {
		use crate::test_support::post_of_type;
		let comment = || parse_comments(&comment_listing(1), "/r/test/comments/link1/a_link/", "op", "", &HashSet::new(), &Preferences::default());
		for post_type in ["image", "video", "gif", "gallery", "link", "self"] {
			let thread = PostTemplate {
				single_thread: true,
				comments: comment(),
				..post_template(post_of_type(post_type, "/img/a.jpg", "/thumb/b/a.jpg").await)
			};
			assert_eq!(highlighted_script_count(&thread.render().unwrap()), 1, "{post_type} thread");
			let whole_post = PostTemplate {
				comments: comment(),
				..post_template(post_of_type(post_type, "/img/a.jpg", "/thumb/b/a.jpg").await)
			};
			assert_eq!(
				highlighted_script_count(&whole_post.render().unwrap()),
				0,
				"{post_type} post without a single comment thread"
			);
		}
	}

	#[tokio::test]
	async fn a11y_markup_post_pages_have_a_skip_link_main_and_one_h1() {
		for (page, html) in post_pages().await {
			crate::html_checks::assert_landmarks(page, &html);
		}
	}

	#[tokio::test]
	async fn template_a11y_and_html_post_pages_are_accessible() {
		for (page, html) in post_pages().await {
			crate::html_checks::assert_accessible(page, &html);
		}
	}

	#[tokio::test]
	async fn opensearch_and_og_meta_urls_are_absolute_when_the_base_is_known() {
		use crate::test_support::{meta_content, post_of_type};
		let image = render_post(post_of_type("image", "/img/abc.jpg", "/thumb/b/xyz.jpg").await, "https://redlib.example");
		assert_eq!(meta_content(&image, "og:image"), Some("https://redlib.example/thumb/b/xyz.jpg"));
		assert_eq!(meta_content(&image, "twitter:image"), Some("https://redlib.example/thumb/b/xyz.jpg"));
		assert_eq!(meta_content(&image, "og:url"), Some("https://redlib.example/r/test/comments/link1/a_link/"));
		assert_eq!(meta_content(&image, "twitter:url"), Some("https://redlib.example/r/test/comments/link1/a_link/"));

		for post_type in ["video", "gif"] {
			let video = render_post(post_of_type(post_type, "/vid/abc/720.mp4", "").await, "https://redlib.example");
			assert_eq!(meta_content(&video, "og:video"), Some("https://redlib.example/vid/abc/720.mp4"), "{post_type}");
		}
	}

	#[tokio::test]
	async fn opensearch_and_og_meta_urls_escape_a_configured_base() {
		use crate::test_support::post_of_type;
		let html = render_post(post_of_type("image", "/img/abc.jpg", "/thumb/b/xyz.jpg").await, "https://libred.example/?a=1&b='2'\"<x>");

		assert!(
			!html.contains("&b='2'") && !html.contains("\"<x>") && !html.contains("<x>"),
			"the base reached the page unescaped: {html}"
		);
		assert!(html.contains("&#38;b=&#39;2&#39;"), "{html}");
	}

	#[tokio::test]
	async fn opensearch_and_og_meta_urls_stay_relative_without_a_base() {
		use crate::test_support::{meta_content, post_of_type};
		let image = render_post(post_of_type("image", "/img/abc.jpg", "/thumb/b/xyz.jpg").await, "");
		assert_eq!(meta_content(&image, "og:image"), Some("/thumb/b/xyz.jpg"));
	}

	#[tokio::test]
	async fn flair_gallery_and_prefs_versioning_links_post_flair_search_encodes_the_flair() {
		let html = PostTemplate {
			comments: Vec::new(),
			post: crate::test_support::flaired_post().await,
			sort: String::new(),
			prefs: Preferences::default(),
			single_thread: false,
			url: "/r/test/comments/f1/flair/".to_string(),
			url_without_query: "/r/test/comments/f1/flair/".to_string(),
			comment_query: String::new(),
			base_url: String::new(),
		}
		.render()
		.unwrap();
		assert_eq!(crate::test_support::flair_search_links(&html), [crate::test_support::AWKWARD_FLAIR_SEARCH_LINK]);
	}

	/// A comment as Reddit sends it: the raw `body` text and its rendered `body_html`.
	fn comment_with(id: &str, body: &str, body_html: &str) -> serde_json::Value {
		json!({ "kind": "t1", "data": {
			"id": id,
			"author": "someone",
			"body": body,
			"body_html": body_html,
			"created_utc": 1_700_000_000.0,
			"parent_id": "t3_abc",
		} })
	}

	fn matching_ids(listing: &serde_json::Value, query: &str) -> Vec<String> {
		let comments = query_comments(listing, "/r/test/comments/abc/post/", "op", "", &HashSet::new(), query, &Preferences::default());
		comments.into_iter().map(|comment| comment.id).collect()
	}

	#[test]
	fn search_encoding_and_comments_matches_text_not_markup() {
		let listing = json!({ "data": { "children": [
			comment_with("link", "[docs](https://example.com/page)", "<div class=\"md\"><p><a href=\"https://example.com/page\">docs</a></p></div>"),
			comment_with("plain", "Rust makes AT&T cheaper", "<div class=\"md\"><p>Rust makes AT&amp;T cheaper</p></div>"),
			comment_with("gone", "[removed]", "<div class=\"md\"><p>[removed]</p></div>"),
		] } });
		for markup in ["href", "class", "md", "div"] {
			assert_eq!(matching_ids(&listing, markup), Vec::<String>::new(), "{markup} is markup, not comment text");
		}
		assert_eq!(matching_ids(&listing, "docs"), ["link"]);
		assert_eq!(matching_ids(&listing, "RUST"), ["plain"], "matching ignores letter case");
		assert_eq!(
			matching_ids(&listing, "undelete"),
			Vec::<String>::new(),
			"a removed comment is not found by its archive link"
		);
	}

	#[test]
	fn comment_prefs_reuse_hides_score_and_awards_for_every_comment() {
		let prefs = Preferences {
			hide_score: "on".to_string(),
			hide_awards: "on".to_string(),
			..Preferences::default()
		};
		let comments = parse_comments(&comment_listing(200), "/r/test/comments/abc/post/", "op", "", &HashSet::new(), &prefs);

		assert_eq!(comments.len(), 200);
		for (index, comment) in comments.iter().enumerate() {
			let score = score_cell(comment);
			assert!(
				score.contains("&#x2022;") && !score.contains("42"),
				"comment {index} should show the hidden-score bullet: {score}"
			);
			assert!(!comment.render().unwrap().contains("class=\"award\""), "comment {index} should hide its awards");
		}
	}

	#[test]
	fn comment_prefs_reuse_shows_score_and_awards_by_default() {
		let comments = parse_comments(&comment_listing(200), "/r/test/comments/abc/post/", "op", "", &HashSet::new(), &Preferences::default());

		assert_eq!(comments.len(), 200);
		for (index, comment) in comments.iter().enumerate() {
			let score = score_cell(comment);
			assert!(score.contains("42") && !score.contains("&#x2022;"), "comment {index} should show its score: {score}");
			assert!(comment.render().unwrap().contains("class=\"award\""), "comment {index} should show its award");
		}
	}

	#[test]
	fn ux_templates_and_settings_feedback_replies_are_a_named_group_not_a_quotation() {
		let listing = json!({ "data": { "children": [{ "kind": "t1", "data": {
			"id": "c1",
			"author": "someone",
			"body": "top",
			"body_html": "<div class=\"md\"><p>top</p></div>",
			"score": 1,
			"created_utc": 1_700_000_000.0,
			"parent_id": "t3_abc",
			"replies": comment_listing(1),
		} }] } });
		let comments = parse_comments(&listing, "/r/test/comments/link1/a_link/", "op", "", &HashSet::new(), &Preferences::default());

		let html = comments[0].render().unwrap();
		assert!(html.contains("role=\"group\"") && html.contains("aria-label=\"Replies to u/someone\""), "{html}");
		assert!(!html.contains("<blockquote"), "replies are not a quotation: {html}");

		let leaf = comments[0].replies[0].render().unwrap();
		assert!(!leaf.contains("role=\"group\""), "a comment without replies has no empty group: {leaf}");
	}

	async fn gallery_page(images: usize) -> String {
		use crate::test_support::post_of_type;
		let item = |caption: &str| GalleryMedia {
			url: "/preview/pre/a.jpg".to_string(),
			width: 100,
			height: 100,
			caption: caption.to_string(),
			outbound_url: String::new(),
		};
		let mut gallery = post_of_type("gallery", "", "").await;
		gallery.gallery = [item("A caption"), item("")].into_iter().take(images).collect();
		render_post(gallery, "")
	}

	#[tokio::test]
	async fn ux_templates_and_settings_feedback_gallery_images_count_and_describe_themselves() {
		let html = gallery_page(2).await;

		assert!(html.contains("1 of 2") && html.contains("2 of 2"), "{html}");
		assert!(html.contains("alt=\"A link (image 1 of 2)\"") && html.contains("alt=\"A link (image 2 of 2)\""), "{html}");
		assert!(html.contains("A caption"), "{html}");
		let empty_paragraph = regex::Regex::new(r"<p>\s*</p>").unwrap();
		assert!(!empty_paragraph.is_match(&html), "an image without a caption gets no empty paragraph: {html}");
	}

	#[tokio::test]
	async fn ux_templates_and_settings_feedback_a_single_image_has_no_counter() {
		let html = gallery_page(1).await;

		assert!(!html.contains("1 of 1"), "{html}");
		assert!(html.contains("alt=\"A link\""), "{html}");
	}
}
