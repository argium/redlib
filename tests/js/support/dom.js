'use strict';
// Fakes for running the scripts in `static/` under `node --test tests/js/`.
// Each script runs in its own `node:vm` context that holds only the globals a test passes in.

const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const STATIC_DIR = path.join(__dirname, '..', '..', '..', 'static');
const SILENT_CONSOLE = { error() {}, log() {}, warn() {} };

/** Runs `static/<name>` in a fresh context holding `globals`, and returns that context. */
function runScript(name, globals) {
	const context = vm.createContext({ console: SILENT_CONSOLE, ...globals });
	vm.runInContext(fs.readFileSync(path.join(STATIC_DIR, name), 'utf8'), context, { filename: name });
	return context;
}

/** A `setTimeout` that only records its calls; `run()` fires everything recorded so far. */
function fakeTimers() {
	const pending = [];
	return {
		setTimeout(callback, delay) {
			pending.push({ callback, delay });
			return pending.length;
		},
		clearTimeout(id) {
			if (pending[id - 1]) pending[id - 1].callback = null;
		},
		delays: () => pending.map((timer) => timer.delay),
		run() {
			for (const timer of pending.splice(0)) timer.callback?.();
		},
	};
}

/** The few DOM members the scripts use; `listeners` collects what `addEventListener` registered. */
function fakeElement(tagName = 'div', properties = {}) {
	const element = {
		tagName,
		attributes: {},
		children: [],
		classNames: new Set(),
		listeners: {},
		parentNode: null,
		textContent: '',
		removed: false,
		classList: {
			add: (name) => element.classNames.add(name),
			contains: (name) => element.classNames.has(name),
		},
		setAttribute(name, value) {
			element.attributes[name] = String(value);
		},
		appendChild(child) {
			element.children.push(child);
			child.parentNode = element;
			return child;
		},
		addEventListener(type, listener) {
			(element.listeners[type] ??= []).push(listener);
		},
		removeEventListener(type, listener) {
			element.listeners[type] = (element.listeners[type] ?? []).filter((existing) => existing !== listener);
		},
		remove() {
			element.removed = true;
		},
		...properties,
	};
	return element;
}

module.exports = { fakeElement, fakeTimers, runScript };
