'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');
const { fakeElement, fakeTimers, runScript } = require('./support/dom');

const EXPORT = 'exported-settings';

/**
 * Loads `copy.js` on the settings page and returns the page after one click on Copy.
 * `clipboard` is `navigator.clipboard` (undefined on plain HTTP); `execCommandWorks` is what
 * `document.execCommand('copy')` returns.
 */
async function clickCopy({ clipboard, execCommandWorks = true }) {
	const timers = fakeTimers();
	const field = fakeElement('input', { value: EXPORT, selected: false, select() { this.selected = true; } });
	const button = fakeElement('button');
	const status = fakeElement('span');
	const elements = { bincode_str: field, copy: button, copy_status: status };
	const execCommands = [];
	const loadListeners = [];

	runScript('copy.js', {
		navigator: { clipboard },
		setTimeout: timers.setTimeout,
		clearTimeout: timers.clearTimeout,
		window: { addEventListener: (type, listener) => loadListeners.push(listener) },
		document: {
			getElementById: (id) => elements[id],
			execCommand: (command) => {
				execCommands.push({ command, fieldSelected: field.selected });
				return execCommandWorks;
			},
		},
	});
	for (const listener of loadListeners) await listener();
	await button.listeners.click[0]();
	return { status, timers, execCommands };
}

test('without navigator.clipboard the field is selected and copied with execCommand', async () => {
	const { status, execCommands } = await clickCopy({ clipboard: undefined });

	assert.deepEqual(execCommands, [{ command: 'copy', fieldSelected: true }]);
	assert.equal(status.textContent, 'Copied');
});

test('with navigator.clipboard the text is written and the status reads Copied', async () => {
	const written = [];
	const { status, execCommands } = await clickCopy({ clipboard: { writeText: async (text) => written.push(text) } });

	assert.deepEqual(written, [EXPORT]);
	assert.deepEqual(execCommands, []);
	assert.equal(status.textContent, 'Copied');
});

test('a refused copy says Copy failed', async () => {
	const { status } = await clickCopy({
		clipboard: { writeText: async () => { throw new Error('denied'); } },
		execCommandWorks: false,
	});

	assert.equal(status.textContent, 'Copy failed');
});

test('the status clears after two seconds', async () => {
	const { status, timers } = await clickCopy({ clipboard: undefined });

	assert.deepEqual(timers.delays(), [2000]);
	timers.run();
	assert.equal(status.textContent, '');
});
