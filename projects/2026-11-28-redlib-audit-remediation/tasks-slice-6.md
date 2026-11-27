# Redlib Audit Remediation, Slice 6 — Task List

- **Status:** Complete
- **Author:** Argi
- **Created:** 2026-11-28
- **Language:** Rust, JavaScript (Node `node:test` for the scripts), Askama HTML templates, CSS
- **Source:** [plan.md](./plan.md), Slice 6 (S6): Theme Contrast, Typography, Responsive Layout & Reading UX

## Summary

Visited post titles, metadata and accent buttons fall below WCAG AA in most themes, text is pixel-locked with no line height, settings and the exit popup break on small screens, and a fixed navbar covers the top of most mobile pages. Comment threads are hard to follow, error and empty pages leave visitors stuck, and settings changes give no feedback. These tasks fix each one against the real stylesheet, templates and routes, and check layout in a browser instead of asserting on CSS text.

## Starting Context

Existing code the tasks build on (verified against `d6f75ac`, the Slice 5 head; do not re-explore):

- `src/main.rs:107` — `style()` served `static/style.css` followed by every `static/themes/*.css` (`ThemeAssets`). It now calls `assets::stylesheet()`, so a test sees the same bytes a browser gets.
- `static/style.css:41-86` — default (dark) and `prefers-color-scheme: light` `:root` blocks; a themes block repeated `.tokyoNight`. Themes set tokens on `body.<name>`; only `light`, `libredditLight` and `gruvboxlight` set `color-scheme` (on `html:has(> .<name>)`).
- `static/style.css` — `font-size` in `px`, no `line-height` on prose, `.prefs-group { height: 35px }`, `.popup-inner { max-height: 500px }` with no scrolling, `body.fixed_navbar` padding patched per page at `<= 800px`, `main { flex-direction: column-reverse }` at `<= 800px`.
- `templates/comment.html:44` — `<blockquote class="replies">`; `templates/utils.html:139-152` — gallery with `alt="Gallery image"` and an always-present caption `<p>`; `templates/error.html`, `templates/subreddit.html:49`, `templates/search.html`, `templates/user.html` — bare `No posts were found.`.
- `src/settings.rs:64` — `set()` redirects to `/settings`; an empty form removes every preference cookie. `encoded_restore()` returns raw error strings as an error page.
- `src/utils.rs:700` — `Preferences::new()` lists themes from `ThemeAssets`; `templates/utils.html:1` — `options` macro capitalises the first letter only.
- `src/html_checks.rs` (test-only) and `tests/js/` with DOM fakes (`node --test tests/js/`) from Slice 5.

Conventions and gotchas:

- Tabs (`rustfmt.toml`). Windows builds need `cmake` and `nasm` on `PATH` (see [plan.md Prerequisites](./plan.md#prerequisites)).
- Reddit answers 403 from this network. Layout and typography are measured in Chromium (Playwright) on pages rendered from the real templates with fixtures, beside the real `/style.css`, in `D:/src/redlib-scratch/s6`. No test asserts on CSS source text.
- The [modern-web-guidance](https://github.com/GoogleChrome/modern-web-guidance) CSS, colour and accessibility guides apply: `color-scheme` on the root element, `prefers-reduced-motion` and `forced-colors` queries, `dvh` units, `:has()` and `color-mix()` are Baseline.
- No new crates.
- Deviations from the plan, recorded here: the plan asks for a CSS-variable offset for `fixed_navbar`. The navbar is 95 px tall on pages with a search box and 45 px on `/error` and the NSFW landing, so one offset cannot fit both. The navbar is `position: sticky` instead, which needs no offset at all.
- On a phone a subreddit's panel is ordered after its posts (the plan allows "collapsed by default or ordered after posts"). The panel's Subscribe and Filter buttons are now below the first page of posts.
- On a phone a subreddit's panel is ordered after its posts (the plan allows "collapsed by default or ordered after posts"). Its Subscribe and Filter buttons are now below the first page of posts.
- The plan names a `.sr-only` class; Slice 5 already added `.visually-hidden`, which this slice reuses.

## Tasks

### Task 1: Every theme reaches WCAG AA and declares its colour scheme

#### Problem

Visited titles fall to 1.2:1 in `icebergDark` and `tokyoNight`, three themes inherit `--visited` from the OS-scheme `:root`, accent buttons and links fail 4.5:1 in most themes (including default `dark` and `light`), metadata is dimmed with `opacity: 0.4–0.5`, and 15 of 18 themes leave the scrollbar and native controls on the OS scheme.

#### Solution

- `src/css_checks.rs` (test-only) parses the served stylesheet, resolves each theme's tokens and reports contrast failures and a missing root `color-scheme`.
- Every theme defines `--visited` and a new `--muted`; accents and surfaces move the smallest step needed.
- Metadata uses `color: var(--muted)` instead of opacity, `.md` links are underlined, each theme sets `color-scheme` on `html:has(> .<name>)`.

#### Notes

- **Location:** `src/css_checks.rs`, `src/assets.rs`, `static/style.css`, `static/themes/*.css`

- **Verify:** `cargo test --locked --lib theme_contrast` passes.

- Boundary: the real stylesheet through `assets::stylesheet()`; underline and scrollbar scheme are checked in a browser.

<details>
<summary>⚙️ <b>Technical details</b></summary>

**Tests (red)**: `assets::tests::theme_contrast_every_served_theme_is_readable_and_declares_its_color_scheme`, plus checker self-tests in `css_checks::tests` (`theme_contrast_checker_*`).

Red failure: 105 lines such as `tokyoNight: --visited on --post is 1.91:1`, `black: --foreground on --accent is 3.21:1`, `icebergDark: does not declare color-scheme`.

**Implementation guidance (green)**: tokens were solved with `scratch/s6/solve.py` (HSL lightness steps until each pair reaches 4.5:1); defaults in `static/style.css` match `dark.css` and `light.css`.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** None

- **Supports:** M6.1, M6.5 (`color-scheme`)

</details>

- [x] Task 1 complete

---

### Task 2: Type scales with the reader's font size and reads at a comfortable measure

#### Problem

Every size is in `px`, so a larger default font size changes nothing; prose has `line-height: normal`; Inter has no `font-display`; the post title is smaller than the headings in its body; wide mode lets prose run past 190 characters per line.

#### Solution

- Font sizes in `rem`; `font-display: swap` with a system-UI fallback stack.
- `line-height: 1.55` on `.md`, `.comment_body`, `.post_body` and the wiki.
- The single-post title is `1.5rem/600`; feed previews cap body headings.
- `.wide` prose is capped at `70ch`.

#### Notes

- **Location:** `static/style.css`

- **Verify:** Playwright probe `typography` shows every text size doubling with `html { font-size: 200% }`, line height 1.55, `font-display: swap`, and `70` characters per prose line in wide mode.

- Boundary: Chromium on rendered fixtures; no Rust test (the plan's CSS-text check is replaced by computed measurements).

<details>
<summary>⚙️ <b>Technical details</b></summary>

Red (before): rem-scaling ratio `1` for every selector, line height `normal`, `font-display: auto`, prose 193 ch in wide mode, post title `16px/500` under a `22px` heading.

</details>

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 1

- **Supports:** M6.2

</details>

- [x] Task 2 complete

---

### Task 3: Settings, popup, navbar and feed order work on small screens

#### Problem

At 667×360 the exit popup's "Yes" button sits below the viewport with nothing to scroll; a fixed navbar covers the top of `/wiki`, `/info`, duplicates and threads at <= 800 px; touch targets are under 44 px; a subreddit's sidebar panel opens above its posts on a phone.

#### Solution

- `.prefs-group` grows with its label; `.popup-inner` scrolls within `90dvh`.
- A fixed navbar is `position: sticky`, and the per-page padding patches go.
- Touch devices (`pointer: coarse`) get 44 px targets for the navbar, tabs, footers, buttons and form rows.
- On <= 800 px a subreddit's panel follows its posts.

#### Notes

- **Location:** `static/style.css`

- **Verify:** Playwright probes `settings_overlap`, `popup`, `navbar`, `touch`, `sidebar_order` report no overlap, a reachable "Yes" button, no covered content, and every target >= 44 px.

- Boundary: Chromium with touch emulation at 320×568, 375×667, 667×360 and 800 px.

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 2

- **Supports:** M6.3

</details>

- [x] Task 3 complete

---

### Task 4: Comment threads announce as groups, show a visible guide line and stay readable when deep

#### Problem

Replies sit in `<blockquote>`, so a screen reader announces every depth as a quotation; the guide line is 1.1:1 against its background and does nothing when clicked; at depth 8 on a phone the text is about 20 characters wide.

#### Solution

- `<div class="replies" role="group" aria-label="Replies to u/name">`, omitted when there are no replies.
- `.line` uses 60% of the text colour over the background (>= 3.3:1 in every theme) with an 8 px unpainted edge on each side as hit area; `collapse_thread.js` collapses the comment when its line is clicked.
- On <= 507 px, a reply from the fifth level on stays level with its parent.

#### Notes

- **Location:** `templates/comment.html`, `static/collapse_thread.js`, `static/style.css`, `src/assets.rs`, `src/main.rs`, `tests/js/collapse_thread.test.js`

- **Verify:** `cargo test --locked ux_templates_and_settings_feedback` and `node --test tests/js/` pass; Playwright probe `thread_depth` shows 27 characters per line at depth 12 on 375 px and 22 at 320 px (11 and 5 before).

- Boundary: rendered template, the script with DOM fakes, and Chromium.

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 3

- **Supports:** M6.4

</details>

- [x] Task 4 complete

---

### Task 5: Error, empty feed and gallery pages tell visitors what to do next

#### Problem

Error pages have no retry; "No posts were found." is a dead end; galleries use `alt="Gallery image"`, show no position and emit an empty caption `<p>`.

#### Solution

- The error page offers "Try again" for the page that failed.
- Empty feeds and searches offer a way out (the default sort, home, all of Reddit).
- Each gallery image has the post title in its alt text and a "1 of N" counter; empty captions are not rendered.

#### Notes

- **Location:** `templates/error.html`, `templates/subreddit.html`, `templates/search.html`, `templates/user.html`, `templates/utils.html`

- **Verify:** `cargo test --locked ux_templates_and_settings_feedback` passes.

- Boundary: rendered templates; no live Reddit.

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 4

- **Supports:** M6.4

</details>

- [x] Task 5 complete

---

### Task 6: Saving, resetting and importing settings answer on the settings page

#### Problem

Saving settings silently reloads the page; an import that fails opens a separate error page with an internal message.

#### Solution

- `POST /settings` redirects to `/settings?status=saved` (or `reset`); a failed import redirects to `/settings?status=import_failed`.
- `/settings` shows a `role="status"` (or `role="alert"` for the failure) message from a fixed vocabulary, never from the query text.
- A "Reset to defaults" button posts an empty form that clears the preference cookies and keeps subscriptions.

#### Notes

- **Location:** `src/settings.rs`, `templates/settings.html`, `static/style.css`

- **Verify:** `cargo test --locked ux_templates_and_settings_feedback` passes.

- Boundary: the handlers called directly and the template rendered offline.

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 5

- **Supports:** M6.4, M6.5 (reset)

</details>

- [x] Task 6 complete

---

### Task 7: Motion and forced colours are respected

#### Problem

Transitions and video autoplay ignore `prefers-reduced-motion`; in forced-colours mode the selected tab loses its fill and icon strokes vanish.

#### Solution

- A `prefers-reduced-motion` block removes transitions; `reduced_motion.js` stops `autoplay` videos and `playHLSVideo.js` skips autoplay.
- A `forced-colors` block restores borders, selected-state outlines and SVG strokes with system colours.

#### Notes

- **Location:** `static/style.css`, `static/reduced_motion.js`, `static/playHLSVideo.js`, `templates/base.html`, `src/assets.rs`, `src/main.rs`, `tests/js/`

- **Verify:** `node --test tests/js/` passes; Playwright with `reducedMotion: 'reduce'` and `forcedColors: 'active'` shows no transition and a visible selected tab.

- Boundary: scripts with fakes, Chromium emulation.

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 6

- **Supports:** M6.5

</details>

- [x] Task 7 complete

---

### Task 8: Monospace stack, readable upvote labels and humane theme names

#### Problem

Code uses `monospace, sans-serif` with no padding, upvote labels are `display: none`, and the theme list shows `libredditBlack`.

#### Solution

- `ui-monospace, "SF Mono", Menlo, Consolas, monospace` and inline-code padding.
- The `Upvotes` label is `.visually-hidden`.
- A `theme_label` filter renders `Libreddit Black`, `Gruvbox Dark`, `Doom One`.

#### Notes

- **Location:** `static/style.css`, `templates/utils.html`, `templates/settings.html`, `src/utils.rs`

- **Verify:** `cargo test --locked themes_and_sr_labels` passes.

- Boundary: rendered templates; the accessibility tree is checked in Chromium.

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Task 7

- **Supports:** M6.5

</details>

- [x] Task 8 complete

---

### Task 9: Integration / Boundary Verification

#### Problem

Each milestone passed on its own; the slice has to pass together.

#### Solution

Run the milestone filters, the script tests, the three repository gates and a Playwright pass, then record the results in Acceptance Evidence below.

#### Notes

- **Location:** repository root

- **Verify:** `node --test tests/js/`, `cargo fmt --check`, `cargo clippy --locked --all-targets -- -D warnings` and `cargo test --locked` all exit 0.

- Boundary: offline suite plus a local browser. Live Reddit stays untouched.

<details>
<summary>📋 <b>Project metadata</b></summary>

- **Blocked by:** Tasks 1–8

- **Supports:** M6.1, M6.2, M6.3, M6.4, M6.5

</details>

- [x] Task 9 complete

---

## Acceptance Evidence

### Checks run

- `cargo fmt --check` clean; `cargo clippy --locked --all-targets -- -D warnings` clean; `cargo test --locked` passes 217 library tests (20 live-Reddit tests ignored), 30 binary tests, 3 `healthcheck` and 2 `startup_config` integration tests offline; `node --test tests/js/` passes 32 tests.
- Filters: `cargo test --locked --lib theme_contrast` (5), `ux_templates_and_settings_feedback` (13), `themes_and_sr_labels` (4).
- Browser checks use Chromium (Playwright 1.64 library, headless) against pages rendered from the real templates with fixtures, beside the real stylesheet (`assets::stylesheet()` output). The scripts, probes, before/after pages and screenshots are in `D:/src/redlib-scratch/s6` (`final_probes.txt` holds every measurement below, before and after).

### Red-green

| Milestone | Test | Observed red failure | Green |
| --- | --- | --- | --- |
| M6.1, M6.5 | `assets::tests::theme_contrast_every_served_theme_is_readable_and_declares_its_color_scheme` | 133 problem lines, e.g. `tokyoNight: --visited on --post is 1.91:1, below 4.5:1`, `black: --foreground on --accent is 3.21:1`, `icebergDark: does not declare color-scheme` | passes |
| M6.1 | `css_checks::tests::theme_contrast_checker_*` (4 self-tests) | the checker reports each defect, a token left to the OS scheme and a body-only `color-scheme`; it accepts a good theme with nested rules | passes |
| M6.4 | `post::tests::ux_templates_and_settings_feedback_replies_are_a_named_group_not_a_quotation` | no `role="group"`; `<blockquote class="replies">` rendered | passes |
| M6.4 | `..._gallery_images_count_and_describe_themselves`, `..._a_single_image_has_no_counter` | `alt="Gallery image"`, no `1 of 2`, empty `<p></p>` | passes |
| M6.4 | `utils::tests::..._error_page_offers_to_try_the_page_again` | `a retry link` missing | passes |
| M6.4 | `..._empty_feed_offers_a_way_out`, `..._empty_search_offers_a_wider_search`, `..._empty_profile_offers_a_way_out` | `left: []`, `right: ["/r/rust?sort=top&t=hour", "/r/rust", "/"]` | passes |
| M6.4 | `settings::tests::ux_templates_and_settings_feedback_*` (saving, reset, failed import, unknown status, unreadable imports, reset button) | `left: "/settings"`, `right: "/settings?status=saved"`; an unreadable import returned `Err("Failed to decode base2048 encoded preferences")` | passes |
| M6.4 | `dos_and_shortlink_limits_encoded_restore_rejects_a_deflate_bomb` (changed) | now expects a redirect to `/settings?status=import_failed` and no `Set-Cookie`; it asserted the raw error text before | passes |
| M6.5 | `utils::tests::themes_and_sr_labels_theme_names_read_as_words` | `left: "system"`, `right: "System"` | passes |
| M6.5 | `settings::tests::themes_and_sr_labels_theme_select_shows_readable_names_and_keeps_file_names_as_values` | option text was `libredditBlack` | passes |
| M6.5 | `subreddit::tests::themes_and_sr_labels_score_label_is_read_by_screen_readers` | `<span class="label">` rendered (hidden by `display: none`) | passes |
| M6.5 | `subreddit::tests::themes_and_sr_labels_reduced_motion_script_loads_only_with_autoplay` | `/reduced_motion.js?v=` missing with autoplay on | passes |
| M6.4 | `tests/js/collapse_thread.test.js` (3) | `ENOENT: static/collapse_thread.js` | passes |
| M6.5 | `tests/js/reduced_motion.test.js` (2), `playHLSVideo.test.js` reduced-motion test | `ENOENT: static/reduced_motion.js`; the HLS player called `play()` once under reduced motion (expected 0) | passes |

`unknown_status_shows_nothing` and `an autoplay video starts by itself` are guards that pass before and after; each has a positive control in a neighbouring test.

### Browser measurements (before, after)

| Check | Before | After |
| --- | --- | --- |
| Text size doubles with `html { font-size: 200% }` (body, nav, titles, `.md`, comments) | ratio 1 for every selector | ratio 2 |
| Prose `line-height` (`.md`, `.comment_body`, `.post_body`) | `normal` | 1.55 |
| `@font-face` `font-display`; font stack | `auto`; `Inter, sans-serif` | `swap`; `Inter, system-ui, -apple-system, "Segoe UI", Roboto, sans-serif` |
| Single-post title / `.md h1` | `16px/500` / `22px` | `24px/600` / `22px` |
| Prose measure in wide mode at 1920 px (content width in ch) | 193 | 70 |
| `.md` link decoration | none | underline |
| Root `color-scheme` under a light OS for the 18 themes | 15 wrong (all dark themes) | 0 wrong; also 0 wrong under a dark OS |
| Settings rows overlapping at 320 px with text at 200% | 11 of 19 rows (rows only overlap once `rem` lets text grow, so this is measured on the Task 2 build) | none |
| Exit popup at 667×360: "Yes" button | bottom at 390 px, popup not scrollable, unreachable | popup scrolls, button reachable |
| Fixed navbar covering the first content (320, 375, 667×360, 800 px; `/r/rust`, thread, `/settings`, `/error`, `/wiki`, NSFW landing, `/info`, duplicates) | covered on thread, wiki, info and duplicates at every width <= 800 | none covered; nav stays at `top: 0` after scrolling 600 px |
| Touch targets under 44 px on `/r/rust`, thread, `/settings`, `/error` at 375 px (block-level controls; inline text links exempt; a checkbox counts through its label) | 22, 27, 58, 2 | 0, 0, 0, 0 |
| First post vs subreddit panel at 375 px (page y) | panel 105, post 563 | post 184, panel 1007 |
| Text width of the reply at depth 12, 375 / 320 px | 11 ch / 5 ch (depth 8: 19 ch / 13 ch) | 27 ch / 22 ch (same width from depth 4 down) |
| Thread line contrast against the page, light / dark | 1.25 / 1.20 | 5.21 / 7.24 (>= 3.3 in every theme by calculation) |
| Clicking a thread line | nothing | collapses the comment |
| Transitions under `prefers-reduced-motion: reduce` | `.post` 0.2 s | 0 s (`.popup` 0.1 s to 0 s) |
| `autoplay` video with `reduced_motion.js` under reduce | n/a | attribute removed |
| Forced colours | selected tab shows only a black fill (`textDecoration: none`), buttons borderless, thread line a thick bar | selected tab underlined, buttons bordered, line 2 px in `CanvasText`, SVG stroke `CanvasText` |
| Horizontal scrolling at 320 px and at 200% text, 13 page shapes | listing tabs overflow | none (`.helper` in a closed `<details>` reports a box but does not scroll) |
| Accessibility tree for a feed post's score | `text: "0"` | `text: 0 Upvotes` |

Screenshots (default dark, `tokyoNight`, `libredditLight` listing at 1280 px; thread and settings at 375 px) are `before_*.png` / `after_*.png` in `D:/src/redlib-scratch/s6/shots`.

### Findings checked against the code

- **#22**: confirmed and fixed. `gruvboxdark`, `gruvboxlight` and `rosebox` had no `--visited`; `style.css` also carried a stale `.tokyoNight` copy, now removed (the theme file is the only definition).
- **#23**: the settings overlap is real only when text grows or labels wrap beyond two lines (not at 320 px with default text); the fixed `35px` is gone either way. The popup and navbar defects reproduced as described.
- **#24, #41, #51, #54**: confirmed as described.
- **#40**: confirmed. Clicking the line collapses a comment; a collapsed comment has no line to click, so the summary row expands it.
- **#42**: copy feedback was Slice 5 (`copy.js`). Settings save, reset and import feedback, gallery, error and empty states are here.

### Not verified, and why

- Real Reddit data: the templates were rendered from fixtures.
- Firefox and Safari: only Chromium was run. `color-mix()`, `:has()`, `dvh`, `align-content` on `summary` and `forced-colors` are Baseline but unseen in other engines.
- A screen reader, a real touch device and real Windows High Contrast palettes: touch (`pointer: coarse`) and forced colours were emulated.
- `font-display: swap` shifts layout when Inter loads because the fallback's metrics differ; no `size-adjust` fallback was added.
- The 8 px hit area around the thread line is lost in forced-colours mode.
- Per-theme contrast covers the text, muted, visited, accent and button pairs named in the plan; `--green` text (moderator names, "all comments") and `--nsfw` were not checked.
- The plan's "CSS inspection confirms `rem` sizing, `max-width: 70ch`" is replaced by computed measurements; no test asserts on CSS text. The plan's "static CSS checks verify `prefers-reduced-motion`/`forced-colors` rules" is replaced by the Chromium emulation above.
