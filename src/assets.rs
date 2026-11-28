//! The scripts Redlib serves itself. Each one is compressed once and carries a version taken from
//! its content, so pages can link it as `/name.js?v=<version>` and browsers may cache that URL
//! for good: a changed script gets a new URL.

use crate::server::StaticBody;
use crate::utils::ThemeAssets;
use hyper::{Body, Request, Response};
use std::collections::hash_map::DefaultHasher;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::LazyLock;

/// Cache lifetime of a URL whose `v` matches the script's content: one year, never revalidated.
const IMMUTABLE_CACHE: &str = "public, max-age=31536000, immutable";

/// Cache lifetime of a URL without the current `v`, e.g. a cached page from before an upgrade.
const SHORT_CACHE: &str = "public, max-age=300";

pub struct Script {
	body: StaticBody,
	version: String,
}

impl Script {
	fn from_bytes(bytes: Vec<u8>) -> Self {
		// shortcut: std's hasher has no stability promise across Rust releases, which only
		// means a toolchain upgrade may change `v` once; upgrade to a real digest if `v` must be stable.
		let mut hasher = DefaultHasher::new();
		bytes.hash(&mut hasher);
		let version = format!("{:016x}", hasher.finish());
		Self {
			body: StaticBody::new(bytes),
			version,
		}
	}

	fn new(source: &'static str) -> Self {
		let mut hasher = DefaultHasher::new();
		source.as_bytes().hash(&mut hasher);
		Self {
			body: StaticBody::new(source.as_bytes()),
			version: format!("{:016x}", hasher.finish()),
		}
	}
}

static STYLESHEET: LazyLock<Script> = LazyLock::new(|| Script::from_bytes(stylesheet().into_bytes()));

static SCRIPTS: LazyLock<HashMap<&'static str, Script>> = LazyLock::new(|| {
	[
		("hls.min.js", include_str!("../static/hls.min.js")),
		("playHLSVideo.js", include_str!("../static/playHLSVideo.js")),
		("highlighted.js", include_str!("../static/highlighted.js")),
		("check_update.js", include_str!("../static/check_update.js")),
		("copy.js", include_str!("../static/copy.js")),
		("collapse_thread.js", include_str!("../static/collapse_thread.js")),
		("reduced_motion.js", include_str!("../static/reduced_motion.js")),
	]
	.into_iter()
	.map(|(name, source)| (name, Script::new(source)))
	.collect()
});

/// `style.css` followed by every theme, as served at `/style.css`.
pub fn stylesheet() -> String {
	let mut css = include_str!("../static/style.css").to_string();
	for file in ThemeAssets::iter() {
		css.push('\n');
		css.push_str(std::str::from_utf8(&ThemeAssets::get(file.as_ref()).unwrap().data).unwrap());
	}
	css
}

/// Builds every script's and stylesheet's compressed form now, instead of during the first request.
pub fn warm() {
	LazyLock::force(&SCRIPTS);
	LazyLock::force(&STYLESHEET);
}

/// The `v` value for the combined stylesheet (`/style.css`), derived from `style.css` and all themes.
pub fn stylesheet_version() -> &'static str {
	STYLESHEET.version.as_str()
}

/// Answers a request for `/style.css`, cached for good only when `v` matches the served CSS.
pub fn stylesheet_response(req: &Request<Body>) -> Result<Response<Body>, String> {
	asset_response(&STYLESHEET, "text/css", req)
}

/// The `v` value for the script `name` (`"hls.min.js"`), for templates. Empty for an unknown name.
pub fn version(name: &str) -> &'static str {
	SCRIPTS.get(name).map_or("", |script| script.version.as_str())
}

/// Answers a request for the script `name`: compressed as the client accepts, cached for good only
/// when the request carries the script's current version.
pub fn script_response(name: &str, req: &Request<Body>) -> Result<Response<Body>, String> {
	let script = SCRIPTS.get(name).ok_or_else(|| format!("No script named {name}"))?;
	asset_response(script, "text/javascript", req)
}

fn asset_response(asset: &Script, content_type: &str, req: &Request<Body>) -> Result<Response<Body>, String> {
	let requests_current_version = url::form_urlencoded::parse(req.uri().query().unwrap_or_default().as_bytes()).any(|(key, value)| key == "v" && value == asset.version);

	let mut res = Response::builder()
		.status(200)
		.header("content-type", content_type)
		.header("cache-control", if requests_current_version { IMMUTABLE_CACHE } else { SHORT_CACHE })
		.body(Body::empty())
		.map_err(|e| e.to_string())?;
	asset.body.apply(req.headers(), &mut res);
	Ok(res)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn static_js_caching_version_follows_the_content() {
		assert_eq!(Script::new("alert(1)").version, Script::new("alert(1)").version);
		assert_ne!(Script::new("alert(1)").version, Script::new("alert(2)").version);
		for name in SCRIPTS.keys() {
			assert!(!version(name).is_empty(), "{name} needs a version for its template link");
		}
		assert_eq!(version("missing.js"), "");
		assert!(!stylesheet_version().is_empty(), "style.css needs a content-derived version");
		assert_ne!(stylesheet_version(), env!("CARGO_PKG_VERSION"));
	}

	#[test]
	fn theme_contrast_every_served_theme_is_readable_and_declares_its_color_scheme() {
		let themes: Vec<String> = ThemeAssets::iter().map(|file| file.trim_end_matches(".css").to_string()).collect();
		assert!(themes.len() > 1, "the themes were not found");
		crate::css_checks::assert_themes_accessible(&stylesheet(), &themes);
	}
}
