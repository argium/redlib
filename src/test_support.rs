//! Fixtures shared by the library's unit tests.

use crate::utils::{parse_post, Post};
use hyper::{Body, Request, Response};
use log::{Level, LevelFilter, Log, Metadata, Record};
use serde_json::json;
use std::future::Future;
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

/// One text post as Reddit sends it with `raw_json=1`: markup in the title is literal text, and
/// the HTML body carries the visitor's `<img src=x>` as `&lt;img src=x&gt;`.
pub async fn feed_posts() -> Vec<Post> {
	let listing = json!({ "data": { "children": [{ "kind": "t3", "data": {
		"id": "p1",
		"subreddit": "rust",
		"author": "ferris",
		"title": "Q&A <b>\"x\"</b>",
		"permalink": "/r/rust/comments/p1/qa/",
		"url": "https://example.com/",
		"created_utc": 1_700_000_000.0,
		"selftext_html": "<div class=\"md\"><p>I typed &lt;img src=x&gt; &amp; left</p></div>",
	} }] } });
	Post::from_listing(&listing).await.unwrap().0
}

/// Runs `handler` on a thread of its own and returns the response text, failing the test when no
/// answer arrives within 5 s. A handler that goes on to fetch from Reddit blocks in the OAuth
/// client's initialiser, which a timeout on the handler's own thread could never interrupt.
pub fn answer_without_fetching<F>(handler: fn(Request<Body>) -> F, request: Request<Body>) -> String
where
	F: Future<Output = Result<Response<Body>, String>> + 'static,
{
	let (sender, receiver) = std::sync::mpsc::channel();
	std::thread::spawn(move || {
		let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
		let response = runtime.block_on(handler(request)).expect("the handler answers with a page");
		let body = runtime.block_on(hyper::body::to_bytes(response.into_body())).unwrap();
		let _ = sender.send(String::from_utf8_lossy(&body).into_owned());
	});
	receiver
		.recv_timeout(std::time::Duration::from_secs(5))
		.expect("the handler must answer without asking Reddit")
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
