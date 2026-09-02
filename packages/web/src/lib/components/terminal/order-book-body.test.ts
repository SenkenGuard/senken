// Renders the real, compiled component through Svelte's own server
// renderer (`svelte/server`) and reads the actual output markup — the same
// `data-*` attributes and text a browser would show — rather than asserting
// on the logic that is supposed to produce them. See
// `../../../scripts/svelte-server-preload.ts` for how a `.svelte` import
// works inside `bun test`.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import OrderBookBody from './order-book-body.svelte';
import type { BookRow } from './order-book';

const noop = () => {};

/** Every state but `ready`/`empty` renders without a timestamp; the two that
 * do show one get it passed explicitly. */
const base = { updatedAt: null, onRetry: noop };

describe('order book states (real DOM output) — capability honesty, not a guessed empty list', () => {
	test('a source that does not report book.supported shows a disabled control with its reason, not an empty list', () => {
		const { body } = render(OrderBookBody, { props: { capability: false, unsupportedTopic: false, failedTopic: false, rows: null, ...base } });
		expect(body).toContain('data-book-state="unsupported"');
		expect(body).toContain('THIS SOURCE DOES NOT REPORT ORDER-BOOK DEPTH');
		// Never a genuinely-empty-book marker for a capability-denied source —
		// the two must stay visibly distinct.
		expect(body).not.toContain('data-book-state="empty"');
		expect(body).not.toContain('ORDER BOOK IS EMPTY');
	});

	test('the WS layer\'s own unsupported answer for this exact topic reads the same as the capability response saying so', () => {
		const { body } = render(OrderBookBody, { props: { capability: true, unsupportedTopic: true, failedTopic: false, rows: null, ...base } });
		expect(body).toContain('data-book-state="unsupported"');
		expect(body).toContain('THIS SOURCE DOES NOT REPORT ORDER-BOOK DEPTH');
	});

	test('capability not yet loaded is a distinct "checking" state, not read as supported or unsupported', () => {
		const { body } = render(OrderBookBody, { props: { capability: undefined, unsupportedTopic: false, failedTopic: false, rows: null, ...base } });
		expect(body).toContain('data-book-state="checking"');
		expect(body).toContain('CHECKING ORDER-BOOK SUPPORT');
	});

	test('a supported source with no snapshot yet is "waiting", distinct from a genuinely empty book', () => {
		const { body } = render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, failedTopic: false, rows: null, ...base } });
		expect(body).toContain('data-book-state="waiting"');
		expect(body).toContain('WAITING FOR DEPTH SNAPSHOT');
	});

	test('a snapshot that arrived and is genuinely empty renders its own stated marker, not silence and not the waiting message', () => {
		const { body } = render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, failedTopic: false, rows: [], ...base } });
		expect(body).toContain('data-book-state="empty"');
		expect(body).toContain('ORDER BOOK IS EMPTY');
		expect(body).not.toContain('WAITING FOR DEPTH SNAPSHOT');
	});

	test('a real snapshot renders every row with its own price, size and side-tinted class', () => {
		const rows: BookRow[] = [
			{ price: '77,927.50', size: '1.5360', side: 'ask', depthPct: 100 },
			{ price: '77,927.40', size: '0.3977', side: 'bid', depthPct: 26 }
		];
		const { body } = render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, failedTopic: false, rows, ...base, updatedAt: '14:32:07' } });
		expect(body).toContain('data-book-state="ready"');
		expect(body).toContain('77,927.50');
		expect(body).toContain('77,927.40');
		expect(body).toContain('1.5360');
		expect(body).toContain('0.3977');
		expect(body).toContain('text-loss');
		expect(body).toContain('text-gain');
		expect(body).toContain('width: 100%');
		expect(body).toContain('width: 26%');
	});

	// The states this component reports must actually read differently, not
	// be the same "nothing to show" text reused under different attribute
	// values.
	test('unsupported, checking, waiting and empty are four distinct labels', () => {
		const strip = (html: string) => html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
		const unsupported = strip(render(OrderBookBody, { props: { capability: false, unsupportedTopic: false, failedTopic: false, rows: null, ...base } }).body);
		const checking = strip(render(OrderBookBody, { props: { capability: undefined, unsupportedTopic: false, failedTopic: false, rows: null, ...base } }).body);
		const waiting = strip(render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, failedTopic: false, rows: null, ...base } }).body);
		const empty = strip(render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, failedTopic: false, rows: [], ...base } }).body);
		expect(new Set([unsupported, checking, waiting, empty]).size).toBe(4);
	});
});

describe('a book that refreshes itself', () => {
	const rows: BookRow[] = [{ price: '77,927.50', size: '1.5360', side: 'ask', depthPct: 100 }];

	test('a live ladder offers nothing to press — the refresh control is gone, not hidden', () => {
		// The panel used to require a press for every new snapshot. Depth now
		// arrives on its own, and a button offering to do what is already
		// happening tells the reader the opposite of the truth.
		const { body } = render(OrderBookBody, {
			props: { capability: true, unsupportedTopic: false, failedTopic: false, rows, updatedAt: '14:32:07', onRetry: noop }
		});
		expect(body).not.toContain('REFRESH');
		expect(body).not.toContain('Refresh order book');
		expect(body).not.toContain('<button');
	});

	test('a live ladder says when it was reported', () => {
		// The one thing a reader cannot otherwise know, and the only honest
		// form of it: a fact about this snapshot, not a promise about the next.
		const { body } = render(OrderBookBody, {
			props: { capability: true, unsupportedTopic: false, failedTopic: false, rows, updatedAt: '14:32:07', onRetry: noop }
		});
		expect(body).toContain('data-book-updated-at="14:32:07"');
		expect(body).toContain('UPDATED 14:32:07');
		expect(body).not.toContain('LIVE');
	});

	test('a book that is genuinely empty is still stamped with when it was reported', () => {
		// An empty book is a real answer from a live stream, not an absence —
		// leaving it unstamped would make it read as one.
		const { body } = render(OrderBookBody, {
			props: { capability: true, unsupportedTopic: false, failedTopic: false, rows: [], updatedAt: '14:32:07', onRetry: noop }
		});
		expect(body).toContain('data-book-state="empty"');
		expect(body).toContain('UPDATED 14:32:07');
	});

	test('a snapshot carrying no usable time still renders its ladder', () => {
		// A missing timestamp is a gap in one line of chrome, never a reason
		// to withhold depth the venue actually reported.
		const { body } = render(OrderBookBody, {
			props: { capability: true, unsupportedTopic: false, failedTopic: false, rows, updatedAt: null, onRetry: noop }
		});
		expect(body).toContain('data-book-state="ready"');
		expect(body).toContain('77,927.50');
		expect(body).toContain('UPDATED —');
	});

	test('the only control this panel has left is the one for a stated failure', () => {
		const control = /<button/;
		const ready = render(OrderBookBody, {
			props: { capability: true, unsupportedTopic: false, failedTopic: false, rows, updatedAt: '14:32:07', onRetry: noop }
		}).body;
		const failed = render(OrderBookBody, {
			props: { capability: true, unsupportedTopic: false, failedTopic: true, rows: null, updatedAt: null, onRetry: noop }
		}).body;
		expect(control.test(ready)).toBe(false);
		expect(control.test(failed)).toBe(true);
	});
});

describe('order book depth that failed to load', () => {
	// A snapshot that failed used to be indistinguishable from one still in
	// flight: the server sent nothing, so the panel waited forever. It is a
	// third fact — the venue supports depth, this attempt did not arrive —
	// and the only one of the three worth offering a retry for.
	test('a failed snapshot is stated, not left waiting', () => {
		const { body } = render(OrderBookBody, {
			props: {
				capability: true,
				unsupportedTopic: false,
				failedTopic: true,
				rows: null,
				updatedAt: null,
				onRetry: () => {}
			}
		});
		expect(body).toContain('data-book-state="failed"');
		expect(body).toContain('COULD NOT LOAD ORDER-BOOK DEPTH');
		expect(body).toContain('aria-label="Retry order book"');
		expect(body).not.toContain('WAITING FOR DEPTH SNAPSHOT');
	});

	test('a venue that does not serve depth is not called a failure', () => {
		const { body } = render(OrderBookBody, {
			props: {
				capability: false,
				unsupportedTopic: false,
				failedTopic: true,
				rows: null,
				updatedAt: null,
				onRetry: () => {}
			}
		});
		expect(body).toContain('data-book-state="unsupported"');
		expect(body).not.toContain('COULD NOT LOAD');
	});
});
