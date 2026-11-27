//! Fixtures shared by the library's unit tests.

use crate::utils::{parse_post, Post};
use log::{Level, LevelFilter, Log, Metadata, Record};
use serde_json::json;
use std::sync::{Mutex, Once};

/// A parsed post. With `hls`, Reddit's video block carries an HLS playlist (so `media.alt_url` is
/// set); without it the post is a plain link.
pub async fn post_fixture(hls: bool) -> Post {
	let data = if hls {
		json!({
			"id": "vid1",
			"subreddit": "test",
			"permalink": "/r/test/comments/vid1/a_video/",
			"secure_media": { "reddit_video": {
				"fallback_url": "https://v.redd.it/abc/DASH_720.mp4",
				"hls_url": "https://v.redd.it/abc/HLSPlaylist.m3u8?a=1",
			} },
		})
	} else {
		json!({ "id": "link1", "subreddit": "test", "permalink": "/r/test/comments/link1/a_link/", "url": "https://example.com/" })
	};
	parse_post(&json!({ "data": data })).await
}

struct CapturingLogger(Mutex<Vec<(Level, String)>>);

impl Log for CapturingLogger {
	fn enabled(&self, _: &Metadata) -> bool {
		true
	}
	fn log(&self, record: &Record) {
		self.0.lock().unwrap().push((record.level(), record.args().to_string()));
	}
	fn flush(&self) {}
}

static LOGGER: CapturingLogger = CapturingLogger(Mutex::new(Vec::new()));

/// Every log record emitted in this test process, all levels included. The process allows one
/// logger, so tests share this one and filter for text unique to them.
pub fn captured_logs() -> &'static Mutex<Vec<(Level, String)>> {
	static INSTALL: Once = Once::new();
	INSTALL.call_once(|| {
		let _ = log::set_logger(&LOGGER);
		log::set_max_level(LevelFilter::Trace);
	});
	&LOGGER.0
}
