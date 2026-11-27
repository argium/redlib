'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');
const { fakeElement, fakeTimers, runScript } = require('./support/dom');

const MP4_URL = 'https://redlib.example/vid/abc/720.mp4';
const NETWORK_ERROR = 'networkError';
const MEDIA_ERROR = 'mediaError';

class FakeHls {
	static Events = { MANIFEST_PARSED: 'manifestParsed', ERROR: 'error' };
	static ErrorType = { NETWORK_ERROR, MEDIA_ERROR };
	static isSupported() {
		return true;
	}

	constructor() {
		this.handlers = {};
		this.calls = [];
		this.levels = [360, 480, 720].map((height) => ({ height, width: height * 2, bitrate: height * 1000 }));
	}
	loadSource() {}
	attachMedia() {}
	on(event, handler) {
		this.handlers[event] = handler;
	}
	startLoad() {
		this.calls.push('startLoad');
	}
	recoverMediaError() {
		this.calls.push('recoverMediaError');
	}
	destroy() {
		this.calls.push('destroy');
	}
	emit(event, data) {
		this.handlers[event](event, data);
	}
	fail(type) {
		this.emit(FakeHls.Events.ERROR, { type, fatal: true });
	}
	count(call) {
		return this.calls.filter((name) => name === call).length;
	}
}

/**
 * Runs `playHLSVideo.js` on a page with one HLS video, then plays it so the script creates its
 * `Hls` instance. `mp4` is the video's MP4 fallback `<source>`, or null when it has none.
 */
function startPlayer({ mp4 = MP4_URL, quality = 'best' } = {}) {
	const timers = fakeTimers();
	const container = fakeElement('div');
	const video = fakeElement('video', { src: '', plays: 0, loads: 0, play() { this.plays++; }, load() { this.loads++; }, querySelectorAll: () => [] });
	const originalVideo = fakeElement('video', {
		canPlayType: () => '',
		cloneNode: () => video,
		querySelector: (selector) => (selector.includes('video/mp4') && mp4 ? { src: mp4 } : null),
	});
	container.appendChild(video);
	originalVideo.parentNode = container;
	const source = { src: 'https://redlib.example/hls/abc/HLSPlaylist.m3u8', type: 'application/vnd.apple.mpegurl', parentNode: originalVideo };
	container.replaceChild = () => {};

	const hlsInstances = [];
	const hlsClass = class extends FakeHls {
		constructor(...args) {
			super(...args);
			hlsInstances.push(this);
		}
	};
	hlsClass.Events = FakeHls.Events;
	hlsClass.ErrorType = FakeHls.ErrorType;
	hlsClass.isSupported = FakeHls.isSupported;

	runScript('playHLSVideo.js', {
		Hls: hlsClass,
		setTimeout: timers.setTimeout,
		document: {
			body: { dataset: { videoQuality: quality } },
			getElementById: () => ({ getAttribute: () => quality }),
			querySelectorAll: () => [source],
			createElement: (tag) => fakeElement(tag),
		},
	});
	video.listeners.play[0]();
	return { hls: hlsInstances[0], video, container, timers };
}

test('network errors are retried three times with doubling waits, then the player is destroyed', () => {
	const { hls, timers } = startPlayer();

	for (let attempt = 0; attempt < 5; attempt++) {
		hls.fail(NETWORK_ERROR);
		timers.run();
	}

	assert.deepEqual(hls.calls, ['startLoad', 'startLoad', 'startLoad', 'destroy']);
});

test('network retries wait one, two then four seconds', () => {
	const { hls, timers } = startPlayer();
	const delays = [];

	for (let attempt = 0; attempt < 3; attempt++) {
		hls.fail(NETWORK_ERROR);
		delays.push(...timers.delays());
		timers.run();
	}

	assert.deepEqual(delays, [1000, 2000, 4000]);
});

test('media errors are recovered three times, then the player is destroyed', () => {
	const { hls, timers } = startPlayer();

	for (let attempt = 0; attempt < 5; attempt++) {
		hls.fail(MEDIA_ERROR);
		timers.run();
	}

	assert.deepEqual(hls.calls, ['recoverMediaError', 'recoverMediaError', 'recoverMediaError', 'destroy']);
});

test('after giving up, the video plays the MP4 source and says so', () => {
	const { hls, video, container, timers } = startPlayer();

	for (let attempt = 0; attempt < 4; attempt++) {
		hls.fail(NETWORK_ERROR);
		timers.run();
	}

	assert.equal(video.src, MP4_URL);
	assert.equal(video.plays, 1);
	const message = container.children.find((child) => child.classNames.has('hls_error'));
	assert.match(message.textContent, /without audio/i);
	assert.equal(message.attributes.role, 'status');
});

test('without an MP4 source the message says the video failed', () => {
	const { hls, video, container, timers } = startPlayer({ mp4: null });

	for (let attempt = 0; attempt < 4; attempt++) {
		hls.fail(NETWORK_ERROR);
		timers.run();
	}

	assert.equal(video.src, 'about:blank');
	const message = container.children.find((child) => child.classNames.has('hls_error'));
	assert.match(message.textContent, /could not be played/i);
});

test('another fatal error type gives up at once', () => {
	const { hls } = startPlayer();

	hls.fail('otherError');

	assert.deepEqual(hls.calls, ['destroy']);
});
