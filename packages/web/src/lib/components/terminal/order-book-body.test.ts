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

describe('order book states (real DOM output) — capability honesty, not a guessed empty list', () => {
	test('a source that does not report book.supported shows a disabled control with its reason, not an empty list', () => {
		const { body } = render(OrderBookBody, { props: { capability: false, unsupportedTopic: false, rows: null, onRefresh: noop } });
		expect(body).toContain('data-book-state="unsupported"');
		expect(body).toContain('THIS SOURCE DOES NOT REPORT ORDER-BOOK DEPTH');
		// Never a genuinely-empty-book marker for a capability-denied source —
		// the two must stay visibly distinct.
		expect(body).not.toContain('data-book-state="empty"');
		expect(body).not.toContain('ORDER BOOK IS EMPTY');
	});

	test('the WS layer\'s own unsupported answer for this exact topic reads the same as the capability response saying so', () => {
		const { body } = render(OrderBookBody, { props: { capability: true, unsupportedTopic: true, rows: null, onRefresh: noop } });
		expect(body).toContain('data-book-state="unsupported"');
		expect(body).toContain('THIS SOURCE DOES NOT REPORT ORDER-BOOK DEPTH');
	});

	test('capability not yet loaded is a distinct "checking" state, not read as supported or unsupported', () => {
		const { body } = render(OrderBookBody, { props: { capability: undefined, unsupportedTopic: false, rows: null, onRefresh: noop } });
		expect(body).toContain('data-book-state="checking"');
		expect(body).toContain('CHECKING ORDER-BOOK SUPPORT');
	});

	test('a supported source with no snapshot yet is "waiting", distinct from a genuinely empty book', () => {
		const { body } = render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, rows: null, onRefresh: noop } });
		expect(body).toContain('data-book-state="waiting"');
		expect(body).toContain('WAITING FOR DEPTH SNAPSHOT');
	});

	test('a snapshot that arrived and is genuinely empty renders its own stated marker, not silence and not the waiting message', () => {
		const { body } = render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, rows: [], onRefresh: noop } });
		expect(body).toContain('data-book-state="empty"');
		expect(body).toContain('ORDER BOOK IS EMPTY');
		expect(body).not.toContain('WAITING FOR DEPTH SNAPSHOT');
	});

	test('a real snapshot renders every row with its own price, size and side-tinted class, plus a refresh control', () => {
		const rows: BookRow[] = [
			{ price: '77,927.50', size: '1.5360', side: 'ask', depthPct: 100 },
			{ price: '77,927.40', size: '0.3977', side: 'bid', depthPct: 26 }
		];
		const { body } = render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, rows, onRefresh: noop } });
		expect(body).toContain('data-book-state="ready"');
		expect(body).toContain('77,927.50');
		expect(body).toContain('77,927.40');
		expect(body).toContain('1.5360');
		expect(body).toContain('0.3977');
		expect(body).toContain('text-loss');
		expect(body).toContain('text-gain');
		expect(body).toContain('width: 100%');
		expect(body).toContain('width: 26%');
		expect(body).toContain('REFRESH');
	});

	// The five states this component reports must actually be five different
	// labels, not the same "nothing to show" text reused under different
	// attribute values.
	test('unsupported, checking, waiting and empty are four distinct labels', () => {
		const strip = (html: string) => html.replace(/<[^>]+>/g, ' ').replace(/\s+/g, ' ').trim();
		const unsupported = strip(render(OrderBookBody, { props: { capability: false, unsupportedTopic: false, rows: null, onRefresh: noop } }).body);
		const checking = strip(render(OrderBookBody, { props: { capability: undefined, unsupportedTopic: false, rows: null, onRefresh: noop } }).body);
		const waiting = strip(render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, rows: null, onRefresh: noop } }).body);
		const empty = strip(render(OrderBookBody, { props: { capability: true, unsupportedTopic: false, rows: [], onRefresh: noop } }).body);
		expect(new Set([unsupported, checking, waiting, empty]).size).toBe(4);
	});
});
