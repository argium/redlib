'use strict';

const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const assert = require('node:assert/strict');
const { test } = require('node:test');
const { fakeElement, fakeTimers, runScript } = require('./support/dom');

// Pull Hls.Events and Hls.ErrorTypes straight from the bundled hls.min.js so the fake cannot
// drift from the library's real API (e.g. ErrorTypes vs ErrorType).
const bundledHls = (() => {
	const ctx = { window: null, self: null, navigator: { userAgent: '' } };
	ctx.window = ctx;
	ctx.self = ctx;
	const source = fs.readFileSync(path.join(__dirname, '..', '..', 'static', 'hls.min.js'), 'utf8');
	vm.runInNewContext(source, ctx, { filename: 'hls.min.js' });
	return ctx.Hls;
})();

const MP4_URL = 'https://redlib.example/vid/abc/720.mp4';
const NETWORK_ERROR = bundledHls.ErrorTypes.NETWORK_ERROR;
const MEDIA_ERROR = bundledHls.ErrorTypes.MEDIA_ERROR;

class FakeHls {
	static Events = bundledHls.Events;
	static ErrorTypes = bundledHls.ErrorTypes;
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
function startPlayer({ mp4 = MP4_URL, quality = 'best', play, autoplay = false, reducedMotion = false } = {}) {
	const timers = fakeTimers();
	const container = fakeElement('div');
	const video = fakeElement('video', {
		src: '',
		plays: 0,
		loads: 0,
		play() {
			this.plays++;
			return play?.();
		},
		load() {
			this.loads++;
		},
		querySelectorAll: () => [],
	});
	const originalVideo = fakeElement('video', {
		canPlayType: () => '',
		cloneNode: () => video,
		querySelector: (selector) => (selector.includes('video/mp4') && mp4 ? { src: mp4 } : null),
	});
	if (autoplay) originalVideo.classNames.add('hls_autoplay');
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
	hlsClass.ErrorTypes = FakeHls.ErrorTypes;
	hlsClass.isSupported = FakeHls.isSupported;

	runScript('playHLSVideo.js', {
		Hls: hlsClass,
		setTimeout: timers.setTimeout,
		window: { matchMedia: (query) => ({ matches: reducedMotion && query === '(prefers-reduced-motion: reduce)' }) },
		document: {
			body: { dataset: { videoQuality: quality } },
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

test('the quality preference comes from the body', () => {
	const { hls } = startPlayer({ quality: 'worst' });

	hls.emit(FakeHls.Events.MANIFEST_PARSED);

	assert.equal(hls.loadLevel, 0);
});

test('the quality selector has an accessible name and goes away when the player gives up', () => {
	const { hls, container, timers } = startPlayer();
	hls.emit(FakeHls.Events.MANIFEST_PARSED);
	const selector = container.children.find((child) => child.tagName === 'select');

	assert.equal(selector.attributes['aria-label'], 'Video quality');

	hls.fail('otherError');
	timers.run();
	assert.equal(selector.removed, true);
});

test('a pending retry does not restart the stream once the player has given up', () => {
	const { hls, timers } = startPlayer();

	hls.fail(NETWORK_ERROR);
	hls.fail('otherError');
	timers.run();

	assert.deepEqual(hls.calls, ['destroy']);
});

test('network and media errors each have their own budget of three', () => {
	const { hls, timers } = startPlayer();

	for (let attempt = 0; attempt < 3; attempt++) {
		hls.fail(NETWORK_ERROR);
		timers.run();
	}
	for (let attempt = 0; attempt < 3; attempt++) {
		hls.fail(MEDIA_ERROR);
	}

	assert.deepEqual(hls.calls, [...Array(3).fill('startLoad'), ...Array(3).fill('recoverMediaError')]);

	hls.fail(MEDIA_ERROR);
	assert.deepEqual(hls.calls.slice(-1), ['destroy']);
});

test('errors after giving up change nothing', () => {
	const { hls, container, timers } = startPlayer();
	hls.fail('otherError');

	hls.fail(NETWORK_ERROR);
	hls.fail(MEDIA_ERROR);
	hls.fail('otherError');
	timers.run();

	assert.deepEqual(hls.calls, ['destroy']);
	assert.equal(container.children.filter((child) => child.classNames.has('hls_error')).length, 1);
});

test('non-fatal errors are ignored', () => {
	const { hls, timers } = startPlayer();

	hls.emit(FakeHls.Events.ERROR, { type: NETWORK_ERROR, fatal: false });
	timers.run();

	assert.deepEqual(hls.calls, []);
});

test('a loaded fragment resets network retries and playback progress resets media retries', () => {
	const { hls, video, timers } = startPlayer();
	video.currentTime = 0;

	for (let burst = 0; burst < 3; burst++) {
		for (let attempt = 0; attempt < 3; attempt++) {
			hls.fail(NETWORK_ERROR);
			timers.run();
			hls.fail(MEDIA_ERROR);
		}
		hls.emit(FakeHls.Events.FRAG_LOADED, {});
		video.currentTime += 2;
		video.listeners.timeupdate[0]();
	}

	assert.equal(hls.count('destroy'), 0);
	assert.equal(hls.count('startLoad'), 9);
	assert.equal(hls.count('recoverMediaError'), 9);
});

test('a downloaded but unplayable fragment cannot renew the media retry budget', () => {
	const { hls, video } = startPlayer();
	video.currentTime = 0;

	for (let attempt = 0; attempt < 4; attempt++) {
		hls.emit(FakeHls.Events.FRAG_LOADED, {});
		hls.fail(MEDIA_ERROR);
	}

	assert.deepEqual(hls.calls, ['recoverMediaError', 'recoverMediaError', 'recoverMediaError', 'destroy']);
});

test('errors with no fragment loaded in between still run out of budget', () => {
	const { hls, timers } = startPlayer();

	for (let attempt = 0; attempt < 4; attempt++) {
		hls.fail(NETWORK_ERROR);
		timers.run();
	}

	assert.equal(hls.count('destroy'), 1);
});

test('a blocked play() on the MP4 fallback is handled, not left as an unhandled rejection', () => {
	let handlers = 0;
	const { hls } = startPlayer({ play: () => ({ catch: () => handlers++ }) });

	hls.fail('otherError');

	assert.equal(handlers, 1);
});

test('an autoplay video starts by itself', () => {
	const { video } = startPlayer({ autoplay: true });

	assert.equal(video.plays, 1);
});

test('with reduced motion preferred, an autoplay video waits for the visitor to press play', () => {
	const { video } = startPlayer({ autoplay: true, reducedMotion: true });

	assert.equal(video.plays, 0);
});
