'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');
const { fakeElement, runScript } = require('./support/dom');

/** Runs `reduced_motion.js` on a page with one video that has `autoplay` and one that has not. */
function runOnPage({ reducedMotion }) {
	const makeVideo = (autoplay) => {
		const video = fakeElement('video', { paused: false, autoplay });
		video.pause = () => (video.paused = true);
		video.removeAttribute = (name) => name === 'autoplay' && (video.autoplay = false);
		return video;
	};
	const autoplaying = makeVideo(true);
	const plain = makeVideo(false);
	runScript('reduced_motion.js', {
		window: { matchMedia: (query) => ({ matches: reducedMotion && query === '(prefers-reduced-motion: reduce)' }) },
		document: { querySelectorAll: (selector) => (selector === 'video[autoplay]' ? [autoplaying] : []) },
	});
	return { autoplaying, plain };
}

test('with reduced motion preferred, an autoplay video is stopped and stops autoplaying', () => {
	const { autoplaying } = runOnPage({ reducedMotion: true });

	assert.equal(autoplaying.paused, true);
	assert.equal(autoplaying.autoplay, false);
});

test('without the preference, an autoplay video is left alone', () => {
	const { autoplaying } = runOnPage({ reducedMotion: false });

	assert.equal(autoplaying.paused, false);
	assert.equal(autoplaying.autoplay, true);
});
