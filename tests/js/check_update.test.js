'use strict';

const assert = require('node:assert/strict');
const { test } = require('node:test');
const { runScript } = require('./support/dom');

const feedOf = (commits) => commits.map((commit) => `<entry><id>tag:github.com,2008:Grit::Commit/${commit}</id></entry>`).join('');

/** Just enough of `DOMParser` to read the `<id>` of each `<entry>` in the feed. */
class FakeDomParser {
	parseFromString(text) {
		const entries = [...text.matchAll(/<entry><id>(.*?)<\/id><\/entry>/g)].map((match) => ({
			getElementsByTagName: () => [{ textContent: match[1] }],
		}));
		return { getElementsByTagName: (tag) => (tag === 'entry' ? entries : []) };
	}
}

/** Runs `check_update.js` on an error page whose instance is at `localCommit`; returns the status text. */
async function statusFor(localCommit, feed) {
	const elements = {
		git_commit: { dataset: { value: localCommit } },
		'update-status': { innerText: '' },
	};
	const context = runScript('check_update.js', {
		DOMParser: FakeDomParser,
		document: { getElementById: (id) => elements[id] },
		fetch: async (url) => (url === '/commits.atom' ? { text: async () => feed } : { json: async () => ({ instances: [] }) }),
		window: { addEventListener() {}, location: { host: 'redlib.example' } },
	});
	await context.checkInstanceUpdateStatus();
	return elements['update-status'].innerText;
}

test('an outdated instance reports how many commits it is behind', async () => {
	const status = await statusFor('ccc', feedOf(['aaa', 'bbb', 'ccc', 'ddd']));

	assert.match(status, /is 2 commits old/);
	assert.doesNotMatch(status, /Error/);
});

test('a commit missing from the feed reports "at least" the feed length', async () => {
	const status = await statusFor('zzz', feedOf(['aaa', 'bbb', 'ccc']));

	assert.match(status, /at least 3 commits old/);
	assert.doesNotMatch(status, /Error/);
});

test('the newest commit reports up to date', async () => {
	assert.match(await statusFor('aaa', feedOf(['aaa', 'bbb'])), /up to date/);
});
