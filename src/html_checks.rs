//! Structural checks for rendered pages. They scan start tags with a regex, which is enough for
//! Askama output: the pages are well-formed apart from the defects these checks look for.
//! Each `assert_*` function panics with the page name and every problem found.

use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static START_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"<([a-zA-Z][a-zA-Z0-9]*)((?:\s+[^\s=>/"']+(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'>]+))?)*)\s*/?>"#).unwrap());
static ATTRIBUTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"([^\s=>/"']+)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'>]+)))?"#).unwrap());
static SVG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<svg\b.*?</svg>").unwrap());
static SVG_TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<title>(.*?)</title>").unwrap());
static ANY_TAG: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<[^>]*>").unwrap());
static FIELDSET_EDGES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"<(/?)(fieldset|legend)\b").unwrap());

const HEAD_ELEMENTS: [&str; 7] = ["title", "meta", "link", "script", "style", "base", "noscript"];

struct Tag {
	name: String,
	attributes: Vec<(String, String, bool)>,
	start: usize,
	end: usize,
}

impl Tag {
	fn attribute(&self, name: &str) -> Option<&str> {
		self.attributes.iter().find(|(attribute, _, _)| attribute == name).map(|(_, value, _)| value.as_str())
	}

	/// The tag as `<name#id>`, falling back to its class, name or href so a report can find it.
	fn description(&self) -> String {
		let hint = if let Some(id) = self.attribute("id") {
			format!("#{id}")
		} else if let Some(class) = self.attribute("class") {
			format!(".{}", class.split_whitespace().collect::<Vec<_>>().join("."))
		} else if let Some(name) = self.attribute("name").or(self.attribute("href")) {
			format!("[{name}]")
		} else {
			String::new()
		};
		format!("<{}{hint}>", self.name)
	}
}

fn without_comments(html: &str) -> String {
	COMMENT.replace_all(html, |comment: &regex::Captures| " ".repeat(comment[0].len())).into_owned()
}

fn start_tags(html: &str) -> Vec<Tag> {
	START_TAG
		.captures_iter(html)
		.map(|captures| {
			let whole = captures.get(0).unwrap();
			let attributes = ATTRIBUTE
				.captures_iter(&captures[2])
				.map(|attribute| {
					let quoted = attribute.get(2).or(attribute.get(3));
					let value = quoted.or(attribute.get(4)).map_or("", |value| value.as_str());
					(attribute[1].to_ascii_lowercase(), value.to_string(), quoted.is_some() || attribute.get(4).is_none())
				})
				.collect();
			Tag {
				name: captures[1].to_ascii_lowercase(),
				attributes,
				start: whole.start(),
				end: whole.end(),
			}
		})
		.collect()
}

/// The text a screen reader would find inside `inner`: SVG icons count only through their `<title>`.
fn accessible_text(inner: &str) -> String {
	let with_svg_titles = SVG.replace_all(inner, |svg: &regex::Captures| {
		SVG_TITLE.captures(&svg[0]).map_or(String::new(), |title| title[1].to_string())
	});
	ANY_TAG.replace_all(&with_svg_titles, "").replace("&nbsp;", " ").trim().to_string()
}

fn inner_html<'a>(html: &'a str, tag: &Tag, closing: &str) -> &'a str {
	let rest = &html[tag.end..];
	&rest[..rest.find(closing).unwrap_or(rest.len())]
}

fn fail_on(page: &str, problems: Vec<String>) {
	assert!(problems.is_empty(), "{page}: {} problem(s):\n  {}", problems.len(), problems.join("\n  "));
}

/// Labels point at real controls, controls have names, `<head>` holds only head elements and
/// every `<p>` is closed.
pub fn assert_accessible(page: &str, html: &str) {
	let html = without_comments(html);
	let tags = start_tags(&html);
	let mut problems = Vec::new();

	for label in tags.iter().filter(|tag| tag.name == "label") {
		let Some(target) = label.attribute("for") else { continue };
		match tags.iter().find(|tag| tag.attribute("id") == Some(target)) {
			None => problems.push(format!("label for=\"{target}\" points at nothing")),
			Some(control) if !["input", "select", "textarea"].contains(&control.name.as_str()) => {
				problems.push(format!("label for=\"{target}\" points at {}, which is not a form control", control.description()))
			}
			Some(_) => {}
		}
	}

	let has_label = |tag: &Tag| {
		tag
			.attribute("id")
			.is_some_and(|id| tags.iter().any(|label| label.name == "label" && label.attribute("for") == Some(id)))
	};
	let has_aria_name = |tag: &Tag| {
		["aria-label", "aria-labelledby"]
			.iter()
			.any(|name| tag.attribute(name).is_some_and(|value| !value.trim().is_empty()))
	};
	for tag in &tags {
		let named = match tag.name.as_str() {
			"select" | "textarea" => has_aria_name(tag) || has_label(tag),
			"input" => match tag.attribute("type").unwrap_or("text") {
				"hidden" => true,
				"submit" | "button" | "reset" => tag.attribute("value").is_some_and(|value| !value.is_empty()) || has_aria_name(tag),
				_ => has_aria_name(tag) || has_label(tag),
			},
			"button" => has_aria_name(tag) || !accessible_text(inner_html(&html, tag, "</button>")).is_empty(),
			"a" if tag.attribute("href").is_some() => {
				has_aria_name(tag) || !accessible_text(inner_html(&html, tag, "</a>")).is_empty() || inner_html(&html, tag, "</a>").contains("alt=\"")
			}
			_ => true,
		};
		if !named {
			problems.push(format!("{} has no accessible name", tag.description()));
		}
	}

	if let (Some(open), Some(close)) = (tags.iter().find(|tag| tag.name == "head"), html.find("</head>")) {
		for tag in tags
			.iter()
			.filter(|tag| tag.start > open.end && tag.start < close && !HEAD_ELEMENTS.contains(&tag.name.as_str()))
		{
			problems.push(format!("{} is inside <head>", tag.description()));
		}
	}

	let (opened, closed) = (tags.iter().filter(|tag| tag.name == "p").count(), html.matches("</p>").count());
	if opened != closed {
		problems.push(format!("<p> opened {opened} times, closed {closed}"));
	}

	fail_on(page, problems);
}

/// Ids are unique, attribute values are quoted, `<legend>` sits in a `<fieldset>`, and no
/// `<button>` is nested in a link.
pub fn assert_valid_markup(page: &str, html: &str) {
	let html = without_comments(html);
	let tags = start_tags(&html);
	let mut problems = Vec::new();

	let mut ids: HashMap<&str, usize> = HashMap::new();
	for id in tags.iter().filter_map(|tag| tag.attribute("id")) {
		*ids.entry(id).or_default() += 1;
	}
	let mut repeated: Vec<_> = ids.into_iter().filter(|(_, count)| *count > 1).collect();
	repeated.sort();
	problems.extend(repeated.into_iter().map(|(id, count)| format!("id=\"{id}\" is used {count} times")));

	for tag in &tags {
		for (name, value, _) in tag.attributes.iter().filter(|(_, _, quoted)| !quoted) {
			problems.push(format!("{} has an unquoted {name}={value}", tag.description()));
		}
	}

	let mut open_fieldsets = 0;
	for edge in FIELDSET_EDGES.captures_iter(&html) {
		match (&edge[1], &edge[2]) {
			("", "fieldset") => open_fieldsets += 1,
			("/", "fieldset") => open_fieldsets -= 1,
			("", "legend") if open_fieldsets == 0 => problems.push("<legend> is outside a <fieldset>".to_string()),
			_ => {}
		}
	}

	for link in tags.iter().filter(|tag| tag.name == "a") {
		if start_tags(inner_html(&html, link, "</a>")).iter().any(|inner| inner.name == "button") {
			problems.push(format!("{} contains a <button>", link.description()));
		}
	}

	fail_on(page, problems);
}

/// The page opens with a skip link to its one `<main id="main-content">` and has one `<h1>`.
pub fn assert_landmarks(page: &str, html: &str) {
	let html = without_comments(html);
	let tags = start_tags(&html);
	let mut problems = Vec::new();

	let first_link = tags.iter().skip_while(|tag| tag.name != "body").find(|tag| tag.name == "a");
	match first_link {
		Some(link) if link.attribute("class").is_some_and(|class| class.split_whitespace().any(|name| name == "skip-link")) && link.attribute("href") == Some("#main-content") => {
		}
		_ => problems.push("the first link in <body> is not a skip link to #main-content".to_string()),
	}

	let targets: Vec<_> = tags.iter().filter(|tag| tag.attribute("id") == Some("main-content")).collect();
	if targets.len() != 1 || targets[0].name != "main" {
		problems.push(format!(
			"expected one <main id=\"main-content\">, found {:?}",
			targets.iter().map(|tag| tag.description()).collect::<Vec<_>>()
		));
	}

	let headings = tags.iter().filter(|tag| tag.name == "h1").count();
	if headings != 1 {
		problems.push(format!("expected one <h1>, found {headings}"));
	}

	fail_on(page, problems);
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::panic::catch_unwind;

	fn problems_of(check: fn(&str, &str), html: &str) -> String {
		let panic = catch_unwind(|| check("page", html)).expect_err("the page has a defect");
		panic.downcast_ref::<String>().cloned().unwrap_or_default()
	}

	#[test]
	fn template_a11y_and_html_checker_reports_each_accessibility_defect() {
		let html = r#"<html><head><title>t</title><div id="video_quality"></div></head><body>
			<label for="missing">a</label><label for="heading">b</label><h2 id="heading">x</h2>
			<select id="s"></select><input type="text" placeholder="only a placeholder">
			<button><svg><path d="M0"/>&rarr;</svg></button><a href="/x"><svg><path d="M0"/></svg></a>
			<p>one<p>two</p></body></html>"#;

		let report = problems_of(assert_accessible, html);

		for expected in [
			"label for=\"missing\" points at nothing",
			"label for=\"heading\" points at <h2#heading>",
			"<select#s> has no accessible name",
			"<input> has no accessible name",
			"<button> has no accessible name",
			"<a[/x]> has no accessible name",
			"<div#video_quality> is inside <head>",
			"<p> opened 2 times, closed 1",
		] {
			assert!(report.contains(expected), "missing {expected:?} in {report}");
		}
	}

	#[test]
	fn template_a11y_and_html_checker_accepts_named_controls() {
		let html = r#"<html><head><title>t</title></head><body>
			<label for="s">Sort</label><select id="s"></select><input type="text" aria-label="Search">
			<button><svg><title>Search</title></svg></button><a href="/x">Home</a><p>one</p></body></html>"#;

		assert_accessible("page", html);
	}

	#[test]
	fn html_markup_and_highlighted_checker_reports_each_markup_defect() {
		let html = r#"<div id="a"></div><div id="a"></div><a href=/x>x</a>
			<legend>loose</legend><a href="/rss"><button>RSS</button></a>
			<fieldset><legend>fine</legend></fieldset>"#;

		let report = problems_of(assert_valid_markup, html);

		for expected in [
			"id=\"a\" is used 2 times",
			"<a[/x]> has an unquoted href=/x",
			"<legend> is outside a <fieldset>",
			"<a[/rss]> contains a <button>",
		] {
			assert!(report.contains(expected), "missing {expected:?} in {report}");
		}
		assert_eq!(report.matches("<legend>").count(), 1, "the legend inside its fieldset is fine: {report}");
	}

	#[test]
	fn a11y_markup_checker_reports_missing_landmarks() {
		let report = problems_of(assert_landmarks, "<body><a href=\"/\">Home</a><main><h2>x</h2></main></body>");

		for expected in ["not a skip link", "expected one <main id=\"main-content\">", "expected one <h1>, found 0"] {
			assert!(report.contains(expected), "missing {expected:?} in {report}");
		}
		assert_landmarks(
			"page",
			"<body><a class=\"skip-link\" href=\"#main-content\">Skip</a><main id=\"main-content\"><h1>x</h1></main></body>",
		);
	}
}
