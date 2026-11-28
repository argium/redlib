'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');
const { fakeElement, runScript } = require('./support/dom');

/** Loads `collapse_thread.js` on a page with one open comment; `click(target)` dispatches a click on a page element. */
function pageWithOneComment() {
	const summary = fakeElement('summary', { scrolledIntoView: [] });
	summary.scrollIntoView = (options) => summary.scrolledIntoView.push(options);
	const details = fakeElement('details', { open: true });
	details.querySelector = (selector) => (selector === 'summary' ? summary : null);
	const comment = fakeElement('div');
	comment.querySelector = (selector) => (selector === 'details.comment_right' ? details : null);
	const line = fakeElement('div');
	line.closest = (selector) => ({ '.line': line, '.comment': comment })[selector] ?? null;
	const body = fakeElement('p');
	body.closest = () => null;

	const listeners = {};
	runScript('collapse_thread.js', { document: { addEventListener: (type, listener) => (listeners[type] = listener) } });
	return { click: (target) => listeners.click({ target }), line, body, details, summary };
}

test('clicking a thread line collapses the comment and brings its header into view', () => {
	const page = pageWithOneComment();

	page.click(page.line);

	assert.equal(page.details.open, false);
	assert.deepEqual(page.summary.scrolledIntoView.map((options) => options.block), ['nearest']);
});

test('clicking the thread line of a collapsed comment expands it without scrolling', () => {
	const page = pageWithOneComment();
	page.details.open = false;

	page.click(page.line);

	assert.equal(page.details.open, true);
	assert.deepEqual(page.summary.scrolledIntoView, []);
});

test('a click that is not on a thread line leaves the comment alone', () => {
	const page = pageWithOneComment();

	page.click(page.body);

	assert.equal(page.details.open, true);
	assert.deepEqual(page.summary.scrolledIntoView, []);
});
