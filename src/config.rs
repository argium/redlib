use serde::{Deserialize, Serialize};
use std::{env::var, fs::read_to_string, sync::LazyLock};

/// This is the local static that is initialized at runtime (technically at
/// first request) and contains the instance settings.
pub static CONFIG: LazyLock<Config> = LazyLock::new(Config::load);

/// This serves as the frontend for an archival API - on removed comments, this URL
/// will be the base of a link, to display removed content (on another site).
pub const DEFAULT_PUSHSHIFT_FRONTEND: &str = "undelete.pullpush.io";

/// Stores the configuration parsed from the environment variables and the
/// config file. `Config::Default()` contains None for each setting.
/// When adding more config settings, add it to `Config::try_load`,
/// `get_setting_from_config`, both below, as well as
/// `instance_info::InstanceInfo.to_string`(), README.md and app.json.
#[derive(Default, Serialize, Deserialize, Clone, Debug)]
pub struct Config {
	#[serde(rename = "REDLIB_SFW_ONLY")]
	#[serde(alias = "LIBREDDIT_SFW_ONLY")]
	pub(crate) sfw_only: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_THEME")]
	#[serde(alias = "LIBREDDIT_DEFAULT_THEME")]
	pub(crate) default_theme: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_FRONT_PAGE")]
	#[serde(alias = "LIBREDDIT_DEFAULT_FRONT_PAGE")]
	pub(crate) default_front_page: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_LAYOUT")]
	#[serde(alias = "LIBREDDIT_DEFAULT_LAYOUT")]
	pub(crate) default_layout: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_WIDE")]
	#[serde(alias = "LIBREDDIT_DEFAULT_WIDE")]
	pub(crate) default_wide: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_COMMENT_SORT")]
	#[serde(alias = "LIBREDDIT_DEFAULT_COMMENT_SORT")]
	pub(crate) default_comment_sort: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_POST_SORT")]
	#[serde(alias = "LIBREDDIT_DEFAULT_POST_SORT")]
	pub(crate) default_post_sort: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_BLUR_SPOILER")]
	#[serde(alias = "LIBREDDIT_DEFAULT_BLUR_SPOILER")]
	pub(crate) default_blur_spoiler: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_SHOW_NSFW")]
	#[serde(alias = "LIBREDDIT_DEFAULT_SHOW_NSFW")]
	pub(crate) default_show_nsfw: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_BLUR_NSFW")]
	#[serde(alias = "LIBREDDIT_DEFAULT_BLUR_NSFW")]
	pub(crate) default_blur_nsfw: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_USE_HLS")]
	#[serde(alias = "LIBREDDIT_DEFAULT_USE_HLS")]
	pub(crate) default_use_hls: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_HIDE_HLS_NOTIFICATION")]
	#[serde(alias = "LIBREDDIT_DEFAULT_HIDE_HLS_NOTIFICATION")]
	pub(crate) default_hide_hls_notification: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_AUTOPLAY_VIDEOS")]
	#[serde(alias = "LIBREDDIT_DEFAULT_AUTOPLAY_VIDEOS")]
	pub(crate) default_autoplay_videos: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_HIDE_AWARDS")]
	#[serde(alias = "LIBREDDIT_DEFAULT_HIDE_AWARDS")]
	pub(crate) default_hide_awards: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_HIDE_SIDEBAR_AND_SUMMARY")]
	#[serde(alias = "LIBREDDIT_DEFAULT_HIDE_SIDEBAR_AND_SUMMARY")]
	pub(crate) default_hide_sidebar_and_summary: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_HIDE_SCORE")]
	#[serde(alias = "LIBREDDIT_DEFAULT_HIDE_SCORE")]
	pub(crate) default_hide_score: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_SUBSCRIPTIONS")]
	#[serde(alias = "LIBREDDIT_DEFAULT_SUBSCRIPTIONS")]
	pub(crate) default_subscriptions: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_FILTERS")]
	#[serde(alias = "LIBREDDIT_DEFAULT_FILTERS")]
	pub(crate) default_filters: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_DISABLE_VISIT_REDDIT_CONFIRMATION")]
	#[serde(alias = "LIBREDDIT_DEFAULT_DISABLE_VISIT_REDDIT_CONFIRMATION")]
	pub(crate) default_disable_visit_reddit_confirmation: Option<String>,

	#[serde(rename = "REDLIB_BANNER")]
	#[serde(alias = "LIBREDDIT_BANNER")]
	pub(crate) banner: Option<String>,

	#[serde(rename = "REDLIB_ROBOTS_DISABLE_INDEXING")]
	#[serde(alias = "LIBREDDIT_ROBOTS_DISABLE_INDEXING")]
	pub(crate) robots_disable_indexing: Option<String>,

	#[serde(rename = "REDLIB_PUSHSHIFT_FRONTEND")]
	#[serde(alias = "LIBREDDIT_PUSHSHIFT_FRONTEND")]
	pub(crate) pushshift: Option<String>,

	#[serde(rename = "REDLIB_ENABLE_RSS")]
	pub(crate) enable_rss: Option<String>,

	#[serde(rename = "REDLIB_FULL_URL")]
	pub(crate) full_url: Option<String>,

	#[serde(rename = "REDLIB_DEFAULT_REMOVE_DEFAULT_FEEDS")]
	pub(crate) default_remove_default_feeds: Option<String>,
}

impl Config {
	/// Load the configuration from the environment variables and the config file.
	/// In the case that there are no environment variables set and there is no
	/// config file, this function returns a Config that contains all None values.
	///
	/// A config file that exists but cannot be read or parsed is an error: ignoring
	/// it would silently drop settings such as `REDLIB_SFW_ONLY`.
	pub fn try_load() -> Result<Self, String> {
		let config = match read_config_file("redlib.toml")? {
			Some(config) => config,
			None => read_config_file("libreddit.toml")?.unwrap_or_default(),
		};

		// This function defines the order of preference - first check for
		// environment variables with "REDLIB", then check the legacy LIBREDDIT
		// option, then check the config, then if all are `None`, return a `None`
		let parse = |key: &str| -> Option<String> {
			// Return the first non-`None` value
			// If all are `None`, return `None`
			let legacy_key = key.replace("REDLIB_", "LIBREDDIT_");
			var(key).ok().or_else(|| var(legacy_key).ok()).or_else(|| get_setting_from_config(key, &config))
		};
		Ok(Self {
			sfw_only: parse("REDLIB_SFW_ONLY"),
			default_theme: parse("REDLIB_DEFAULT_THEME"),
			default_front_page: parse("REDLIB_DEFAULT_FRONT_PAGE"),
			default_layout: parse("REDLIB_DEFAULT_LAYOUT"),
			default_post_sort: parse("REDLIB_DEFAULT_POST_SORT"),
			default_wide: parse("REDLIB_DEFAULT_WIDE"),
			default_comment_sort: parse("REDLIB_DEFAULT_COMMENT_SORT"),
			default_blur_spoiler: parse("REDLIB_DEFAULT_BLUR_SPOILER"),
			default_show_nsfw: parse("REDLIB_DEFAULT_SHOW_NSFW"),
			default_blur_nsfw: parse("REDLIB_DEFAULT_BLUR_NSFW"),
			default_use_hls: parse("REDLIB_DEFAULT_USE_HLS"),
			default_hide_hls_notification: parse("REDLIB_DEFAULT_HIDE_HLS_NOTIFICATION"),
			default_autoplay_videos: parse("REDLIB_DEFAULT_AUTOPLAY_VIDEOS"),
			default_hide_awards: parse("REDLIB_DEFAULT_HIDE_AWARDS"),
			default_hide_sidebar_and_summary: parse("REDLIB_DEFAULT_HIDE_SIDEBAR_AND_SUMMARY"),
			default_hide_score: parse("REDLIB_DEFAULT_HIDE_SCORE"),
			default_subscriptions: parse("REDLIB_DEFAULT_SUBSCRIPTIONS"),
			default_filters: parse("REDLIB_DEFAULT_FILTERS"),
			default_disable_visit_reddit_confirmation: parse("REDLIB_DEFAULT_DISABLE_VISIT_REDDIT_CONFIRMATION"),
			banner: parse("REDLIB_BANNER"),
			robots_disable_indexing: parse("REDLIB_ROBOTS_DISABLE_INDEXING"),
			pushshift: parse("REDLIB_PUSHSHIFT_FRONTEND"),
			enable_rss: parse("REDLIB_ENABLE_RSS"),
			full_url: parse("REDLIB_FULL_URL"),
			default_remove_default_feeds: parse("REDLIB_DEFAULT_REMOVE_DEFAULT_FEEDS"),
		})
	}

	/// Like [`Config::try_load`], but stops the process on a config file error.
	/// This backs the `CONFIG` static, which is first read during startup.
	pub fn load() -> Self {
		Self::try_load().unwrap_or_else(|e| {
			eprintln!("{e}");
			std::process::exit(1);
		})
	}
}

/// Reads and parses a config file. A file that does not exist is `Ok(None)`.
fn read_config_file(name: &str) -> Result<Option<Config>, String> {
	match read_to_string(name) {
		Ok(contents) => toml::from_str(&contents).map(Some).map_err(|e| format!("Failed to parse {name}: {e}")),
		Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
			// A dangling symlink reports NotFound from read_to_string even though the config entry exists.
			if std::fs::symlink_metadata(name).is_ok() {
				Err(format!("Failed to read {name}: {e}"))
			} else {
				Ok(None)
			}
		}
		Err(e) => Err(format!("Failed to read {name}: {e}")),
	}
}

fn get_setting_from_config(name: &str, config: &Config) -> Option<String> {
	match name {
		"REDLIB_SFW_ONLY" => config.sfw_only.clone(),
		"REDLIB_DEFAULT_THEME" => config.default_theme.clone(),
		"REDLIB_DEFAULT_FRONT_PAGE" => config.default_front_page.clone(),
		"REDLIB_DEFAULT_LAYOUT" => config.default_layout.clone(),
		"REDLIB_DEFAULT_COMMENT_SORT" => config.default_comment_sort.clone(),
		"REDLIB_DEFAULT_POST_SORT" => config.default_post_sort.clone(),
		"REDLIB_DEFAULT_BLUR_SPOILER" => config.default_blur_spoiler.clone(),
		"REDLIB_DEFAULT_SHOW_NSFW" => config.default_show_nsfw.clone(),
		"REDLIB_DEFAULT_BLUR_NSFW" => config.default_blur_nsfw.clone(),
		"REDLIB_DEFAULT_USE_HLS" => config.default_use_hls.clone(),
		"REDLIB_DEFAULT_HIDE_HLS_NOTIFICATION" => config.default_hide_hls_notification.clone(),
		"REDLIB_DEFAULT_WIDE" => config.default_wide.clone(),
		"REDLIB_DEFAULT_AUTOPLAY_VIDEOS" => config.default_autoplay_videos.clone(),
		"REDLIB_DEFAULT_HIDE_AWARDS" => config.default_hide_awards.clone(),
		"REDLIB_DEFAULT_HIDE_SIDEBAR_AND_SUMMARY" => config.default_hide_sidebar_and_summary.clone(),
		"REDLIB_DEFAULT_HIDE_SCORE" => config.default_hide_score.clone(),
		"REDLIB_DEFAULT_SUBSCRIPTIONS" => config.default_subscriptions.clone(),
		"REDLIB_DEFAULT_FILTERS" => config.default_filters.clone(),
		"REDLIB_DEFAULT_DISABLE_VISIT_REDDIT_CONFIRMATION" => config.default_disable_visit_reddit_confirmation.clone(),
		"REDLIB_BANNER" => config.banner.clone(),
		"REDLIB_ROBOTS_DISABLE_INDEXING" => config.robots_disable_indexing.clone(),
		"REDLIB_PUSHSHIFT_FRONTEND" => config.pushshift.clone(),
		"REDLIB_ENABLE_RSS" => config.enable_rss.clone(),
		"REDLIB_FULL_URL" => config.full_url.clone(),
		"REDLIB_DEFAULT_REMOVE_DEFAULT_FEEDS" => config.default_remove_default_feeds.clone(),
		_ => None,
	}
}

/// Retrieves setting from environment variable or config file.
pub fn get_setting(name: &str) -> Option<String> {
	get_setting_from_config(name, &CONFIG)
}

#[cfg(test)]
mod tests {
	use super::*;
	use {sealed_test::prelude::*, std::fs::write};

	#[test]
	fn test_deserialize() {
		// Must handle empty input
		let result = toml::from_str::<Config>("");
		assert!(result.is_ok(), "Error: {}", result.unwrap_err());
	}

	#[test]
	#[sealed_test(env = [("REDLIB_SFW_ONLY", "on")])]
	fn test_env_var() {
		assert!(crate::utils::sfw_only())
	}

	#[test]
	#[sealed_test]
	fn startup_config_and_bind_rejects_malformed_toml() {
		write("redlib.toml", "REDLIB_SFW_ONLY = ").unwrap();
		let error = Config::try_load().unwrap_err();
		assert!(error.contains("redlib.toml"), "{error}");
	}

	#[test]
	#[sealed_test]
	fn startup_config_and_bind_rejects_unreadable_config() {
		// A directory with the config's name exists but cannot be read as a file.
		std::fs::create_dir("redlib.toml").unwrap();
		let error = Config::try_load().unwrap_err();
		assert!(error.contains("redlib.toml"), "{error}");
	}

	#[test]
	#[sealed_test]
	fn startup_config_and_bind_rejects_malformed_legacy_file() {
		write("libreddit.toml", "REDLIB_SFW_ONLY = ").unwrap();
		assert!(Config::try_load().unwrap_err().contains("libreddit.toml"));
	}

	#[test]
	#[sealed_test]
	fn startup_config_and_bind_accepts_missing_files() {
		let config = Config::try_load().unwrap();
		assert_eq!(config.sfw_only, None);
	}

	#[test]
	#[sealed_test]
	fn startup_config_and_bind_fails_on_dangling_symlink() {
		#[cfg(unix)]
		let created = std::os::unix::fs::symlink("missing-target.toml", "redlib.toml");
		#[cfg(windows)]
		let created = std::os::windows::fs::symlink_file("missing-target.toml", "redlib.toml");
		// On Windows without Developer Mode / SeCreateSymbolicLinkPrivilege, skip rather than fail setup.
		if created.is_err() {
			return;
		}
		let error = Config::try_load().expect_err("a dangling redlib.toml symlink must fail startup");
		assert!(error.contains("redlib.toml"), "{error}");
	}

	#[test]
	#[sealed_test]
	fn startup_config_and_bind_reads_valid_file() {
		write("redlib.toml", r#"REDLIB_SFW_ONLY = "on""#).unwrap();
		assert_eq!(Config::try_load().unwrap().sfw_only, Some("on".to_string()));
	}

	#[test]
	#[sealed_test]
	fn test_config() {
		let config_to_write = r#"REDLIB_DEFAULT_COMMENT_SORT = "best""#;
		write("redlib.toml", config_to_write).unwrap();
		assert_eq!(get_setting("REDLIB_DEFAULT_COMMENT_SORT"), Some("best".into()));
	}

	#[test]
	#[sealed_test]
	fn test_config_legacy() {
		let config_to_write = r#"LIBREDDIT_DEFAULT_COMMENT_SORT = "best""#;
		write("libreddit.toml", config_to_write).unwrap();
		assert_eq!(get_setting("REDLIB_DEFAULT_COMMENT_SORT"), Some("best".into()));
	}

	#[test]
	#[sealed_test(env = [("LIBREDDIT_SFW_ONLY", "on")])]
	fn test_env_var_legacy() {
		assert!(crate::utils::sfw_only())
	}

	#[test]
	#[sealed_test(env = [("REDLIB_DEFAULT_COMMENT_SORT", "top")])]
	fn test_env_config_precedence() {
		let config_to_write = r#"REDLIB_DEFAULT_COMMENT_SORT = "best""#;
		write("redlib.toml", config_to_write).unwrap();
		assert_eq!(get_setting("REDLIB_DEFAULT_COMMENT_SORT"), Some("top".into()))
	}

	#[test]
	#[sealed_test(env = [("REDLIB_DEFAULT_COMMENT_SORT", "top")])]
	fn test_alt_env_config_precedence() {
		let config_to_write = r#"REDLIB_DEFAULT_COMMENT_SORT = "best""#;
		write("redlib.toml", config_to_write).unwrap();
		assert_eq!(get_setting("REDLIB_DEFAULT_COMMENT_SORT"), Some("top".into()))
	}

	#[test]
	#[sealed_test(env = [("REDLIB_DEFAULT_SUBSCRIPTIONS", "news+bestof")])]
	fn test_default_subscriptions() {
		assert_eq!(get_setting("REDLIB_DEFAULT_SUBSCRIPTIONS"), Some("news+bestof".into()));
	}

	#[test]
	#[sealed_test(env = [("REDLIB_DEFAULT_FILTERS", "news+bestof")])]
	fn test_default_filters() {
		assert_eq!(get_setting("REDLIB_DEFAULT_FILTERS"), Some("news+bestof".into()));
	}

	#[test]
	#[sealed_test(env = [("REDLIB_DEFAULT_AUTOPLAY_VIDEOS", "on")])]
	fn wiki_info_and_config_autoplay_default_is_applied_to_preferences() {
		let request = hyper::Request::get("/").body(hyper::Body::empty()).unwrap();
		assert_eq!(crate::utils::Preferences::new(&request).autoplay_videos, "on");
	}

	#[test]
	fn wiki_info_and_config_app_json_env_names_are_config_keys() {
		let config_keys = serde_json::to_value(Config::default()).unwrap();
		let app_json: serde_json::Value = serde_json::from_str(include_str!("../app.json")).unwrap();
		let unknown: Vec<_> = app_json["env"]
			.as_object()
			.unwrap()
			.keys()
			.filter(|name| config_keys.get(name.as_str()).is_none())
			.collect();
		assert!(unknown.is_empty(), "app.json names variables that Config does not read: {unknown:?}");
	}

	#[test]
	#[sealed_test]
	fn test_pushshift() {
		let config_to_write = r#"REDLIB_PUSHSHIFT_FRONTEND = "https://api.pushshift.io""#;
		write("redlib.toml", config_to_write).unwrap();
		assert!(get_setting("REDLIB_PUSHSHIFT_FRONTEND").is_some());
		assert_eq!(get_setting("REDLIB_PUSHSHIFT_FRONTEND"), Some("https://api.pushshift.io".into()));
	}
}
