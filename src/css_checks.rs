//! Test-only reader for the served stylesheet. It resolves the colour tokens each theme gives the
//! page and reports WCAG 2.x contrast failures and themes that do not declare a `color-scheme`.

use regex::Regex;
use std::collections::HashMap;

/// WCAG AA for normal text. Redlib uses the same ratio for every text pair, large or small.
const MIN_CONTRAST: f64 = 4.5;

/// Every token a theme must define itself: a missing one is inherited from the OS-scheme `:root`
/// block, which is light or dark depending on the visitor and not on the chosen theme.
const TOKENS: [&str; 9] = ["text", "muted", "visited", "accent", "foreground", "background", "post", "outside", "highlighted"];

/// (text token, surfaces the text appears on).
const TEXT_ON_SURFACES: [(&str, &[&str]); 5] = [
	("text", &["background", "post", "outside", "foreground", "highlighted"]),
	// `.comment_body_filtered` is muted text on a highlighted comment.
	("muted", &["background", "post", "outside", "foreground", "highlighted"]),
	("visited", &["post", "foreground"]),
	("accent", &["background", "post", "outside", "foreground"]),
	// Selected tabs and buttons draw `--foreground` text on an `--accent` fill.
	("foreground", &["accent"]),
];

struct Rule {
	selectors: Vec<String>,
	/// The enclosing at-rule preludes, e.g. `@media (prefers-color-scheme: light)`.
	conditions: Vec<String>,
	declarations: Vec<(String, String)>,
}

/// Splits `css` at the top level of `text` into `prelude { body }` blocks and `name: value;` declarations,
/// ignoring `;`, `{` and `}` inside strings and parentheses (the stylesheet embeds `data:` SVGs).
fn parse_block(text: &str, conditions: &[String], rules: &mut Vec<Rule>) -> Vec<(String, String)> {
	let mut declarations = Vec::new();
	let mut current = String::new();
	let (mut quote, mut parentheses) = (None, 0);
	let mut chars = text.char_indices();
	while let Some((index, character)) = chars.next() {
		match (quote, character) {
			(Some(delimiter), _) if character == delimiter => quote = None,
			(Some(_), _) => {}
			(None, '"' | '\'') => quote = Some(character),
			(None, '(') => parentheses += 1,
			(None, ')') => parentheses -= 1,
			(None, '{') if parentheses == 0 => {
				let end = matching_brace(text, index);
				let prelude = current.trim().to_string();
				let inner = &text[index + 1..end];
				if prelude.starts_with('@') {
					let mut nested = conditions.to_vec();
					nested.push(prelude);
					parse_block(inner, &nested, rules);
				} else {
					let declarations = parse_block(inner, conditions, rules);
					let selectors = prelude.split(',').map(|selector| selector.trim().to_string()).collect();
					rules.push(Rule {
						selectors,
						conditions: conditions.to_vec(),
						declarations,
					});
				}
				current.clear();
				while chars.next().is_some_and(|(next, _)| next < end) {}
				continue;
			}
			(None, ';') if parentheses == 0 => {
				push_declaration(&mut declarations, &current);
				current.clear();
				continue;
			}
			_ => {}
		}
		current.push(character);
	}
	push_declaration(&mut declarations, &current);
	declarations
}

fn matching_brace(text: &str, open: usize) -> usize {
	let (mut depth, mut quote) = (0, None);
	for (index, character) in text[open..].char_indices() {
		match (quote, character) {
			(Some(delimiter), _) if character == delimiter => quote = None,
			(Some(_), _) => {}
			(None, '"' | '\'') => quote = Some(character),
			(None, '{') => depth += 1,
			(None, '}') => {
				depth -= 1;
				if depth == 0 {
					return open + index;
				}
			}
			_ => {}
		}
	}
	panic!("unbalanced braces in the stylesheet");
}

fn push_declaration(declarations: &mut Vec<(String, String)>, text: &str) {
	if let Some((name, value)) = text.split_once(':') {
		declarations.push((name.trim().to_string(), value.trim().to_string()));
	}
}

fn parse(css: &str) -> Vec<Rule> {
	let without_comments = Regex::new(r"(?s)/\*.*?\*/").unwrap().replace_all(css, "");
	let mut rules = Vec::new();
	parse_block(&without_comments, &[], &mut rules);
	rules
}

/// The declarations a page gets from rules matching `applies`, later rules overriding earlier ones.
fn merged(rules: &[Rule], applies: impl Fn(&Rule) -> bool) -> HashMap<String, String> {
	rules.iter().filter(|rule| applies(rule)).flat_map(|rule| rule.declarations.clone()).collect()
}

fn names_class(theme: &str) -> impl Fn(&Rule) -> bool {
	let class = Regex::new(&format!(r"(^|[^\w-])\.{theme}([^\w-]|$)")).unwrap();
	move |rule| rule.conditions.is_empty() && rule.selectors.iter().any(|selector| class.is_match(selector))
}

/// The rule targets the page's root element when the body has the theme's class. The viewport's scrollbars
/// follow the root element's `color-scheme`; the body's own value only reaches the controls inside it.
fn styles_root_of(theme: &str) -> impl Fn(&Rule) -> bool {
	let selector = Regex::new(&format!(r"^html:has\(>\s*\.{theme}\)$")).unwrap();
	move |rule| rule.conditions.is_empty() && rule.selectors.iter().any(|candidate| selector.is_match(candidate))
}

fn is_root(rule: &Rule) -> bool {
	rule.selectors.iter().any(|selector| selector == ":root")
}

fn is_light_os_scheme(rule: &Rule) -> bool {
	rule.conditions == ["@media (prefers-color-scheme: light)"]
}

fn colour(value: &str) -> Option<[f64; 3]> {
	let hex = match value.trim() {
		"white" => "ffffff",
		"black" => "000000",
		"aqua" => "00ffff",
		other => other.strip_prefix('#')?,
	};
	let digits: Vec<u32> = match hex.len() {
		3 => hex.chars().map(|digit| digit.to_digit(16).map(|value| value * 17)).collect::<Option<_>>()?,
		6 => (0..3)
			.map(|channel| u32::from_str_radix(&hex[channel * 2..channel * 2 + 2], 16).ok())
			.collect::<Option<_>>()?,
		_ => return None,
	};
	Some([digits[0] as f64, digits[1] as f64, digits[2] as f64].map(|channel| channel / 255.0))
}

fn luminance([red, green, blue]: [f64; 3]) -> f64 {
	let linear = |channel: f64| if channel <= 0.03928 { channel / 12.92 } else { ((channel + 0.055) / 1.055).powf(2.4) };
	0.2126 * linear(red) + 0.7152 * linear(green) + 0.0722 * linear(blue)
}

fn contrast(first: [f64; 3], second: [f64; 3]) -> f64 {
	let (lighter, darker) = (luminance(first).max(luminance(second)), luminance(first).min(luminance(second)));
	(lighter + 0.05) / (darker + 0.05)
}

fn check_tokens(label: &str, tokens: &HashMap<String, String>, problems: &mut Vec<String>) {
	let mut colours = HashMap::new();
	for token in TOKENS {
		match tokens.get(&format!("--{token}")).map(|value| (value, colour(value))) {
			None => problems.push(format!("{label}: --{token} is not defined by the theme")),
			Some((value, None)) => problems.push(format!("{label}: --{token} is {value:?}, which this check cannot read")),
			Some((_, Some(rgb))) => {
				colours.insert(token, rgb);
			}
		}
	}
	for (text, surfaces) in TEXT_ON_SURFACES {
		for surface in surfaces {
			if let (Some(&text_rgb), Some(&surface_rgb)) = (colours.get(text), colours.get(surface)) {
				let ratio = contrast(text_rgb, surface_rgb);
				if ratio < MIN_CONTRAST {
					problems.push(format!("{label}: --{text} on --{surface} is {ratio:.2}:1, below {MIN_CONTRAST}:1"));
				}
			}
		}
	}
}

fn check_color_scheme(label: &str, declarations: &HashMap<String, String>, problems: &mut Vec<String>) {
	if !matches!(declarations.get("color-scheme").map(String::as_str), Some("light" | "dark")) {
		problems.push(format!("{label}: does not declare color-scheme: light or dark on the root element"));
	}
}

/// Checks `css` (the served stylesheet) for each named theme, and for the default theme a visitor gets
/// from their OS (dark, or light under `prefers-color-scheme: light`). Panics with every problem found.
pub fn assert_themes_accessible(css: &str, themes: &[String]) {
	let rules = parse(css);
	let mut problems = Vec::new();

	let system_dark = merged(&rules, |rule| rule.conditions.is_empty() && is_root(rule));
	let mut system_light = system_dark.clone();
	system_light.extend(merged(&rules, |rule| is_light_os_scheme(rule) && is_root(rule)));
	for (label, tokens) in [("system (dark OS)", &system_dark), ("system (light OS)", &system_light)] {
		check_tokens(label, tokens, &mut problems);
		check_color_scheme(label, tokens, &mut problems);
	}

	for theme in themes {
		check_tokens(theme, &merged(&rules, names_class(theme)), &mut problems);
		check_color_scheme(theme, &merged(&rules, styles_root_of(theme)), &mut problems);
	}

	assert!(problems.is_empty(), "theme accessibility problems:\n{}", problems.join("\n"));
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::panic::catch_unwind;

	const GOOD_THEME: &str = ".good { --text: white; --muted: #bbb; --visited: #bbb; --accent: #f08090; --foreground: #111; --background: #000; --post: #111; --outside: #181818; --highlighted: #222; }
		html:has(> .good) { color-scheme: dark; }";

	fn report_for(css: &str, theme: &str) -> String {
		let panic = catch_unwind(|| assert_themes_accessible(&format!("{SYSTEM}{css}"), &[theme.to_string()])).expect_err("the theme has a defect");
		panic.downcast_ref::<String>().cloned().unwrap_or_default()
	}

	const SYSTEM: &str = ":root, .dark { --text: white; --muted: #bbb; --visited: #bbb; --accent: #f08090; --foreground: #111; --background: #000; --post: #111; --outside: #181818; --highlighted: #222; color-scheme: dark; }
		@media (prefers-color-scheme: light) { :root { --text: black; --muted: #444; --visited: #444; --accent: #a01020; --foreground: #eee; --background: #fff; --post: #f5f5f5; --outside: #eaeaea; --highlighted: #fff; color-scheme: light; } }
		.x { background: url(\"data:image/svg+xml;utf8,<svg a='1;2'>{}</svg>\"); }";

	#[test]
	fn theme_contrast_checker_accepts_a_good_theme_and_nested_rules() {
		assert_themes_accessible(
			&format!("{SYSTEM}{GOOD_THEME} @media (max-width: 1px) {{ body {{ .good {{ color: red; }} }} }}"),
			&["good".to_string()],
		);
	}

	#[test]
	fn theme_contrast_checker_reports_each_defect() {
		let css =
			".bad { --text: white; --muted: #333; --accent: #000; --foreground: #111; --background: #000; --post: #111; --outside: #181818; --highlighted: #222; --visited: #fo0; }";

		let report = report_for(css, "bad");

		for expected in [
			"bad: --muted on --post is 1.",
			"bad: --accent on --background is 1.00:1",
			"bad: --foreground on --accent is 1.",
			"bad: --visited is \"#fo0\", which this check cannot read",
			"bad: does not declare color-scheme",
		] {
			assert!(report.contains(expected), "missing {expected:?} in {report}");
		}
		assert!(!report.contains("--text"), "white text passes everywhere: {report}");
	}

	#[test]
	fn theme_contrast_checker_wants_the_color_scheme_on_the_root_element() {
		let body_only = GOOD_THEME.replace("html:has(> .good)", ".good");

		let report = report_for(&body_only, "good");

		assert!(report.contains("good: does not declare color-scheme"), "{report}");
	}

	#[test]
	fn theme_contrast_checker_reports_a_token_the_theme_leaves_to_the_os_scheme() {
		let css = ".inherits { --text: white; color-scheme: dark; }";

		let report = report_for(css, "inherits");

		assert!(report.contains("inherits: --visited is not defined by the theme"), "{report}");
	}
}
