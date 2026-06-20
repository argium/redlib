#![allow(clippy::cmp_owned)]
use crate::utils::{self, catch_random, error, filter_posts, format_num, format_url, get_filters, param, redirect, setting, template, val, Post, Preferences};
use crate::{
	client::json,
	server::RequestExt,
	subreddit::{can_access_quarantine, quarantine},
};
use askama::Template;
use hyper::{Body, Request, Response};
use regex::Regex;
use std::sync::LazyLock;
use url::form_urlencoded;

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

impl SearchParams {
	fn posts_query(&self) -> String {
		encode_params(&[("q", &self.q), ("sort", &self.sort), ("t", &self.t)])
	}

	fn sr_user_query(&self) -> String {
		encode_params(&[("q", &self.q), ("sort", &self.sort), ("t", &self.t), ("type", "sr_user")])
	}

	fn previous_page_query(&self) -> String {
		self.page_query("before", &self.before)
	}

	fn next_page_query(&self) -> String {
		self.page_query("after", &self.after)
	}

	fn page_query(&self, cursor_name: &str, cursor: &str) -> String {
		let mut params = form_urlencoded::Serializer::new(String::new());
		params.append_pair("q", &self.q);
		if !self.restrict_sr.is_empty() {
			params.append_pair("restrict_sr", &self.restrict_sr);
		}
		params.append_pair("sort", &self.sort);
		params.append_pair("t", &self.t);
		params.append_pair(cursor_name, cursor);
		params.finish()
	}
}

fn encode_params(pairs: &[(&str, &str)]) -> String {
	let mut params = form_urlencoded::Serializer::new(String::new());
	for &(key, value) in pairs {
		params.append_pair(key, value);
	}
	params.finish()
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
			params: SearchParams {
				q: query.clone(),
				sort,
				t: param(&path, "t").unwrap_or_default(),
				before: param(&path, "after").unwrap_or_default(),
				after: String::new(),
				restrict_sr: param(&path, "restrict_sr").unwrap_or_default(),
				typed,
			},
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
					params: SearchParams {
						q: query.clone(),
						sort,
						t: param(&path, "t").unwrap_or_default(),
						before: param(&path, "after").unwrap_or_default(),
						after,
						restrict_sr: param(&path, "restrict_sr").unwrap_or_default(),
						typed,
					},
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

async fn search_subreddits(q: &str, typed: &str) -> Vec<Subreddit> {
	let limit = if typed == "sr_user" { "50" } else { "3" };
	let subreddit_search_path = format!("/subreddits/search.json?{}", encode_params(&[("q", q), ("limit", limit)]));

	// Send a request to the url
	json(subreddit_search_path, false).await.unwrap_or_default()["data"]["children"]
		.as_array()
		.map(ToOwned::to_owned)
		.unwrap_or_default()
		.iter()
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
	use super::SearchParams;

	fn params() -> SearchParams {
		SearchParams {
			q: "flair_name:\"A/B & C+\"".to_string(),
			sort: "top".to_string(),
			t: "week".to_string(),
			before: "t3_prev/1".to_string(),
			after: "t3_next/2".to_string(),
			restrict_sr: "on".to_string(),
			typed: String::new(),
		}
	}

	#[test]
	fn search_params_url_encode_links() {
		let params = params();

		assert_eq!(params.posts_query(), "q=flair_name%3A%22A%2FB+%26+C%2B%22&sort=top&t=week");
		assert_eq!(params.sr_user_query(), "q=flair_name%3A%22A%2FB+%26+C%2B%22&sort=top&t=week&type=sr_user");
		assert_eq!(
			params.previous_page_query(),
			"q=flair_name%3A%22A%2FB+%26+C%2B%22&restrict_sr=on&sort=top&t=week&before=t3_prev%2F1"
		);
		assert_eq!(
			params.next_page_query(),
			"q=flair_name%3A%22A%2FB+%26+C%2B%22&restrict_sr=on&sort=top&t=week&after=t3_next%2F2"
		);
	}
}
