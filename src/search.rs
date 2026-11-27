#![allow(clippy::cmp_owned)]
use crate::utils::{self, catch_random, error, filter_posts, format_num, format_url, get_filters, param, redirect, setting, template, val, Post, Preferences};
use crate::{
	client::json,
	server::RequestExt,
	subreddit::{can_access_quarantine, quarantine},
};
use askama::Template;
use hyper::{Body, Request, Response};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use regex::Regex;
use std::sync::LazyLock;

// STRUCTS
struct SearchParams {
	q: String,
	sort: String,
	t: String,
	before: String,
	after: String,
	restrict_sr: String,
	typed: String,
}

// STRUCTS
struct Subreddit {
	name: String,
	url: String,
	icon: String,
	description: String,
	subscribers: (String, String),
}

#[derive(Template)]
#[template(path = "search.html")]
struct SearchTemplate {
	posts: Vec<Post>,
	subreddits: Vec<Subreddit>,
	sub: String,
	params: SearchParams,
	prefs: Preferences,
	url: String,
	/// Whether the subreddit itself is filtered.
	is_filtered: bool,
	/// Whether all fetched posts are filtered (to differentiate between no posts fetched in the first place,
	/// and all fetched posts being filtered).
	all_posts_filtered: bool,
	/// Whether all posts were hidden because they are NSFW (and user has disabled show NSFW)
	all_posts_hidden_nsfw: bool,
	no_posts: bool,
}

/// Regex matched against search queries to determine if they are reddit urls.
static REDDIT_URL_MATCH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^https?://([^\./]+\.)*reddit.com/").unwrap());

// SERVICES
pub async fn find(req: Request<Body>) -> Result<Response<Body>, String> {
	// This ensures that during a search, no NSFW posts are fetched at all
	let nsfw_results = if setting(&req, "show_nsfw") == "on" && !utils::sfw_only() {
		"&include_over_18=on"
	} else {
		""
	};
	let uri_path = req.uri().path().replace("+", "%2B");
	let path = format!("{}.json?{}{}&raw_json=1", uri_path, req.uri().query().unwrap_or_default(), nsfw_results);
	let mut query = param(&path, "q").unwrap_or_default();
	query = REDDIT_URL_MATCH.replace(&query, "").to_string();

	if query.is_empty() {
		return Ok(redirect("/"));
	}

	if query.starts_with("r/") || query.starts_with("user/") {
		return Ok(redirect(&format!("/{query}")));
	}

	if query.starts_with("R/") {
		return Ok(redirect(&format!("/r{}", &query[1..])));
	}

	if query.starts_with("u/") || query.starts_with("U/") {
		return Ok(redirect(&format!("/user{}", &query[1..])));
	}

	let sub = req.param("sub").unwrap_or_default();
	let quarantined = can_access_quarantine(&req, &sub);
	// Handle random subreddits
	if let Ok(random) = catch_random(&sub, "/find").await {
		return Ok(random);
	}

	let typed = param(&path, "type").unwrap_or_default();

	let sort = param(&path, "sort").unwrap_or_else(|| "relevance".to_string());
	let filters = get_filters(&req);

	// If search is not restricted to this subreddit, show other subreddits in search results
	let subreddits = if param(&path, "restrict_sr").is_none() {
		let mut subreddits = search_subreddits(&query, &typed).await;
		subreddits.retain(|s| !filters.contains(s.name.as_str()));
		subreddits
	} else {
		Vec::new()
	};

	let url = String::from(req.uri().path_and_query().map_or("", |val| val.as_str()));

	// If all requested subs are filtered, we don't need to fetch posts.
	if sub.split('+').all(|s| filters.contains(s)) {
		Ok(template(&SearchTemplate {
			posts: Vec::new(),
			subreddits,
			sub,
			params: search_params(&path, query, sort, typed, String::new()),
			prefs: Preferences::new(&req),
			url,
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
				Ok(template(&SearchTemplate {
					posts,
					subreddits,
					sub,
					params: search_params(&path, query, sort, typed, after),
					prefs: Preferences::new(&req),
					url,
					is_filtered: false,
					all_posts_filtered,
					all_posts_hidden_nsfw,
					no_posts,
				}))
			}
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
}

/// The search as the result page shows it. `q` stays raw text: the template HTML-escapes it where
/// displayed and percent-encodes it inside links.
fn search_params(path: &str, query: String, sort: String, typed: String, after: String) -> SearchParams {
	SearchParams {
		q: query,
		sort,
		t: param(path, "t").unwrap_or_default(),
		before: param(path, "after").unwrap_or_default(),
		after,
		restrict_sr: param(path, "restrict_sr").unwrap_or_default(),
		typed,
	}
}

fn subreddit_search_path(q: &str, typed: &str) -> String {
	let limit = if typed == "sr_user" { "50" } else { "3" };
	format!("/subreddits/search.json?q={}&limit={limit}", utf8_percent_encode(q, NON_ALPHANUMERIC))
}

async fn search_subreddits(q: &str, typed: &str) -> Vec<Subreddit> {
	// Send a request to the url
	let listing = json(subreddit_search_path(q, typed), false).await.unwrap_or_default();
	subreddit_results(&listing, utils::sfw_only())
}

/// Reads the communities of a subreddit search listing. An SFW-only instance leaves out the NSFW ones.
fn subreddit_results(listing: &serde_json::Value, sfw_only: bool) -> Vec<Subreddit> {
	listing["data"]["children"]
		.as_array()
		.map(ToOwned::to_owned)
		.unwrap_or_default()
		.iter()
		.filter(|subreddit| !(sfw_only && subreddit["data"]["over18"].as_bool().unwrap_or_default()))
		.map(|subreddit| {
			// For each subreddit from subreddit list
			// Fetch subreddit icon either from the community_icon or icon_img value
			let icon = subreddit["data"]["community_icon"].as_str().map_or_else(|| val(subreddit, "icon_img"), ToString::to_string);

			Subreddit {
				name: val(subreddit, "display_name"),
				url: val(subreddit, "url"),
				icon: format_url(&icon),
				description: val(subreddit, "public_description"),
				subscribers: format_num(subreddit["data"]["subscribers"].as_f64().unwrap_or_default() as i64),
			}
		})
		.collect::<Vec<Subreddit>>()
}

#[cfg(test)]
mod tests {
	use super::{search_params, subreddit_results, subreddit_search_path, SearchParams, SearchTemplate, Subreddit};
	use crate::test_support::post_fixture;
	use crate::utils::Preferences;
	use askama::Template;

	async fn rendered_results(use_hls: &str, hls_post: bool) -> String {
		SearchTemplate {
			posts: vec![post_fixture(hls_post).await],
			subreddits: Vec::new(),
			sub: String::new(),
			params: SearchParams {
				q: "cats".to_string(),
				sort: "relevance".to_string(),
				t: String::new(),
				before: String::new(),
				after: String::new(),
				restrict_sr: String::new(),
				typed: "all".to_string(),
			},
			prefs: Preferences {
				use_hls: use_hls.to_string(),
				..Preferences::default()
			},
			url: "/search?q=cats".to_string(),
			is_filtered: false,
			all_posts_filtered: false,
			all_posts_hidden_nsfw: false,
			no_posts: false,
		}
		.render()
		.unwrap()
	}

	#[tokio::test]
	async fn static_js_caching_search_loads_hls_only_for_hls_posts() {
		let with_hls = rendered_results("on", true).await;
		let hls = format!("/hls.min.js?v={}\" defer", crate::assets::version("hls.min.js"));
		let player = format!("/playHLSVideo.js?v={}\" defer", crate::assets::version("playHLSVideo.js"));
		assert!(with_hls.contains(&hls) && with_hls.contains(&player), "content-versioned, deferred HLS scripts expected");
		assert!(with_hls.find(&hls) < with_hls.find(&player), "hls.min.js must come first so playHLSVideo.js can use Hls");
		assert!(!rendered_results("on", false).await.contains("hls.min.js"), "no HLS post, no hls.min.js");
		assert!(!rendered_results("off", true).await.contains("hls.min.js"), "use_hls off, no hls.min.js");
	}

	const AWKWARD_QUERY: &str = r#"C++ & AT&T #1 100% "x""#;
	const AWKWARD_QUERY_ENCODED: &str = "C%2B%2B%20%26%20AT%26T%20%231%20100%25%20%22x%22";

	fn render_search(q: &str, typed: &str) -> String {
		search_template(q, typed).render().unwrap()
	}

	fn search_template(q: &str, typed: &str) -> SearchTemplate {
		SearchTemplate {
			posts: Vec::new(),
			subreddits: vec![Subreddit {
				name: "cpp".to_string(),
				url: "/r/cpp/".to_string(),
				icon: String::new(),
				description: String::new(),
				subscribers: ("1".to_string(), "1".to_string()),
			}],
			sub: String::new(),
			params: SearchParams {
				q: q.to_string(),
				sort: "relevance".to_string(),
				t: "all".to_string(),
				before: "t3_before".to_string(),
				after: "t3_after".to_string(),
				restrict_sr: "on".to_string(),
				typed: typed.to_string(),
			},
			prefs: Preferences::default(),
			url: "/search".to_string(),
			is_filtered: false,
			all_posts_filtered: false,
			all_posts_hidden_nsfw: false,
			no_posts: true,
		}
	}

	#[test]
	fn search_encoding_and_comments_subreddit_lookup_encodes_the_query() {
		assert_eq!(
			subreddit_search_path(AWKWARD_QUERY, ""),
			format!("/subreddits/search.json?q={AWKWARD_QUERY_ENCODED}&limit=3")
		);
		assert_eq!(subreddit_search_path("rust", "sr_user"), "/subreddits/search.json?q=rust&limit=50");
	}

	#[test]
	fn search_encoding_and_comments_pagination_cursors_are_encoded() {
		let html = SearchTemplate {
			params: SearchParams {
				q: "rust".to_string(),
				sort: "relevance".to_string(),
				t: "all".to_string(),
				before: "t3_a&q=evil".to_string(),
				after: "t3_b#x".to_string(),
				restrict_sr: "on".to_string(),
				typed: "all".to_string(),
			},
			..search_template("rust", "all")
		}
		.render()
		.unwrap();
		assert!(html.contains("&before=t3_a%26q%3Devil\""), "PREV cursor must stay one parameter");
		assert!(html.contains("&after=t3_b%23x\""), "NEXT cursor must not start a fragment");
	}

	#[test]
	fn search_encoding_and_comments_params_keep_the_query_as_raw_text() {
		let path = "/search.json?q=x&t=week&restrict_sr=on&after=t3_abc&raw_json=1";
		let params = search_params(path, r#"say "hi""#.to_string(), "top".to_string(), "all".to_string(), "t3_next".to_string());
		assert_eq!(params.q, r#"say "hi""#);
		assert_eq!((params.t.as_str(), params.restrict_sr.as_str()), ("week", "on"));
		assert_eq!((params.before.as_str(), params.after.as_str()), ("t3_abc", "t3_next"));
	}

	#[test]
	fn search_encoding_and_comments_links_carry_the_encoded_query() {
		// "all" shows the "more subreddit results" link and the PREV/NEXT footer; "sr_user" shows the "back" link.
		for (typed, links) in [("all", 3), ("sr_user", 1)] {
			let html = render_search(AWKWARD_QUERY, typed);
			assert_eq!(html.matches("?q=").count(), links, "{typed}: link count");
			assert_eq!(
				html.matches(&format!("?q={AWKWARD_QUERY_ENCODED}&")).count(),
				links,
				"{typed}: every link carries the encoded query"
			);
		}
	}

	#[test]
	fn search_encoding_and_comments_query_is_html_escaped_once_where_shown() {
		let html = render_search(AWKWARD_QUERY, "all");
		assert!(html.contains(r#"value="C++ &#38; AT&#38;T #1 100% &#34;x&#34;""#), "search box");
		assert!(html.contains("search results - C++ &#38; AT&#38;T #1 100% &#34;x&#34;</title>"), "title");
		assert!(!html.contains("&quot;") && !html.contains("&amp;quot;"), "no hand-made entities");
	}

	fn listing() -> serde_json::Value {
		serde_json::json!({ "data": { "children": [
			{ "data": { "display_name": "family", "url": "/r/family/", "over18": false } },
			{ "data": { "display_name": "adult", "url": "/r/adult/", "over18": true } },
		] } })
	}

	#[test]
	fn sfw_only_search_results_leave_out_nsfw_communities() {
		let names: Vec<_> = subreddit_results(&listing(), true).into_iter().map(|s| s.name).collect();
		assert_eq!(names, ["family"]);
	}

	#[test]
	fn search_results_keep_nsfw_communities_when_the_instance_allows_them() {
		let names: Vec<_> = subreddit_results(&listing(), false).into_iter().map(|s| s.name).collect();
		assert_eq!(names, ["family", "adult"]);
	}
}
