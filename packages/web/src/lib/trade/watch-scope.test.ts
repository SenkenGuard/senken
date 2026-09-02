import { describe, expect, test } from 'bun:test';
import { WatchScopes } from './watch-scope';

describe('WatchScopes', () => {
	test('with nothing watching, a tick would fetch only the active account', () => {
		expect(new WatchScopes().current()).toBe('active');
	});

	test('one broad watcher widens the tick', () => {
		const scopes = new WatchScopes();
		scopes.add('all');
		expect(scopes.current()).toBe('all');
	});

	test('a narrow watcher does not widen it', () => {
		// The whole point: the chart panel must not make every tick fetch
		// every account.
		const scopes = new WatchScopes();
		scopes.add('active');
		expect(scopes.current()).toBe('active');
	});

	test('releasing the broad watcher narrows the very next tick', () => {
		const scopes = new WatchScopes();
		const release = scopes.add('all');
		scopes.add('active');
		expect(scopes.current()).toBe('all');

		release();

		// Closing the engine page must stop the chart page from fetching
		// every account on the very next tick.
		expect(scopes.current()).toBe('active');
	});

	test('a narrow watcher releasing leaves a broad one alone', () => {
		const scopes = new WatchScopes();
		scopes.add('all');
		const releaseNarrow = scopes.add('active');

		releaseNarrow();

		expect(scopes.current()).toBe('all');
	});

	test('two broad watchers both have to release before it narrows', () => {
		const scopes = new WatchScopes();
		const first = scopes.add('all');
		const second = scopes.add('all');

		first();
		expect(scopes.current()).toBe('all');

		second();
		expect(scopes.current()).toBe('active');
	});

	test('releasing twice cannot corrupt the count for other watchers', () => {
		// A component whose cleanup runs twice would otherwise drive the
		// count negative and silently narrow another screen's refresh.
		const scopes = new WatchScopes();
		const release = scopes.add('all');
		scopes.add('all');

		release();
		release();

		expect(scopes.current()).toBe('all');
	});
});
