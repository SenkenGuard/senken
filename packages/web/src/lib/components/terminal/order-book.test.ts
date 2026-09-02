import { describe, expect, test } from 'bun:test';
import type { WsBookLevel } from '$lib/api/ws-frames';
import { bookRowsFromSnapshot, foldBookEvent, formatBookTime, NO_BOOK_YET } from './order-book';

describe('bookRowsFromSnapshot — real depth, no synthetic ladder', () => {
	test('a single-level snapshot formats price/size at the wire scale and orders asks above bids', () => {
		// Exactly the recorded shape `crates/api/src/ws.rs`'s `Book` frame sends.
		const bids: WsBookLevel[] = [{ price: 779_274, size: 39_772_851 }];
		const asks: WsBookLevel[] = [{ price: 779_275, size: 153_603_807 }];

		const rows = bookRowsFromSnapshot(bids, asks, 1, 8);

		expect(rows).toEqual([
			{ price: '77,927.50', size: '1.5360', side: 'ask', depthPct: 100 },
			{ price: '77,927.40', size: '0.3977', side: 'bid', depthPct: 26 }
		]);
	});

	test('asks are reversed so the farthest ask paints first (top) and the best ask sits nearest the spread', () => {
		// Best-first ascending on the wire, per `bookFrameFor`'s own doc.
		const asks: WsBookLevel[] = [
			{ price: 100, size: 1 },
			{ price: 101, size: 1 },
			{ price: 102, size: 1 }
		];
		const rows = bookRowsFromSnapshot([], asks, 0, 0);
		expect(rows.map((r) => r.price)).toEqual(['102.00', '101.00', '100.00']);
	});

	test('bids keep their best-first (descending) wire order — the best bid sits nearest the spread already', () => {
		const bids: WsBookLevel[] = [
			{ price: 99, size: 1 },
			{ price: 98, size: 1 },
			{ price: 97, size: 1 }
		];
		const rows = bookRowsFromSnapshot(bids, [], 0, 0);
		expect(rows.map((r) => r.price)).toEqual(['99.00', '98.00', '97.00']);
	});

	test('depthPct is proportional to the largest size in the whole snapshot, both sides combined', () => {
		const bids: WsBookLevel[] = [{ price: 1, size: 10 }];
		const asks: WsBookLevel[] = [{ price: 2, size: 40 }];
		const rows = bookRowsFromSnapshot(bids, asks, 0, 0);
		const ask = rows.find((r) => r.side === 'ask');
		const bid = rows.find((r) => r.side === 'bid');
		expect(ask?.depthPct).toBe(100);
		expect(bid?.depthPct).toBe(25);
	});

	test('a level far smaller than the largest still gets a visible minimum fill, not a near-zero sliver', () => {
		const bids: WsBookLevel[] = [{ price: 1, size: 1 }];
		const asks: WsBookLevel[] = [{ price: 2, size: 100_000 }];
		const rows = bookRowsFromSnapshot(bids, asks, 0, 0);
		const bid = rows.find((r) => r.side === 'bid');
		expect(bid?.depthPct).toBeGreaterThanOrEqual(4);
	});

	test('a genuinely empty book on both sides returns an empty row list, not a placeholder row', () => {
		expect(bookRowsFromSnapshot([], [], 1, 8)).toEqual([]);
	});
});


describe('when a snapshot says it was reported', () => {
	// The panel prints this instead of a "LIVE" badge, so it has to be right
	// about the one thing it claims: the venue's own instant for this book.
	test('a venue timestamp renders as a zero-padded wall clock', () => {
		// Built from a local-time constructor so the assertion is about the
		// formatting, not about the machine's zone.
		const at = new Date(2026, 8, 1, 9, 5, 7);
		expect(formatBookTime(at.getTime())).toBe('09:05:07');
	});

	test('every part is padded, so the row never changes width under the reader', () => {
		// A locale that drops the leading zero shifts the whole ladder header
		// once a second at 09:59:59.
		const at = new Date(2026, 8, 1, 23, 59, 59);
		expect(formatBookTime(at.getTime())).toBe('23:59:59');
		expect(formatBookTime(new Date(2026, 8, 1, 0, 0, 0).getTime())).toHaveLength(8);
	});

	test('no usable time reads as none, never as midnight', () => {
		// `0` is the epoch, not "now" and not "unknown" — a book stamped
		// 00:00:00 would look like a real, very stale snapshot.
		expect(formatBookTime(0)).toBeNull();
		expect(formatBookTime(null)).toBeNull();
		expect(formatBookTime(undefined)).toBeNull();
		expect(formatBookTime(Number.NaN)).toBeNull();
		expect(formatBookTime(Number.POSITIVE_INFINITY)).toBeNull();
		expect(formatBookTime(-1)).toBeNull();
	});
});

describe('what the panel knows about a topic, folded from its frames', () => {
	const topic = 'book:okx-spot:BTC-USDT';
	// `receivedAt` is `WsEvent`'s own field; a literal missing it is not the
	// shape the store actually hands the panel.
	const at = 1_788_253_213_400;
	const bookEvent = (t: string, price: number) => ({
		type: 'book' as const,
		receivedAt: at,
		payload: {
			topic: t,
			bids: [{ price, size: 1 }],
			asks: [{ price: price + 1, size: 1 }],
			price_scale: 1,
			qty_scale: 1,
			ts: 1_788_253_213_356
		}
	});
	const failedEvent = (t: string) => ({ type: 'failed' as const, receivedAt: at, payload: { topic: t } });
	const unsupportedEvent = (t: string) => ({
		type: 'unsupported' as const,
		receivedAt: at,
		payload: { topic: t }
	});

	test('a snapshot arriving clears a previous failure', () => {
		// The property the whole live-book change rests on. The server's poll
		// loop recovers on its own, so a failure flag that only ever gets set
		// would leave "could not load depth" on screen over a book that is
		// arriving again every second — and nothing the reader did would
		// clear it except pressing retry, which is the control this change
		// exists to stop depending on.
		const failed = foldBookEvent(NO_BOOK_YET, failedEvent(topic), topic);
		expect(failed.failed).toBe(true);

		const recovered = foldBookEvent(failed, bookEvent(topic, 100), topic);
		expect(recovered.failed).toBe(false);
		expect(recovered.snapshot?.bids[0].price).toBe(100);
	});

	test('a newer snapshot replaces the last one outright — frames are whole books, not deltas', () => {
		const first = foldBookEvent(NO_BOOK_YET, bookEvent(topic, 100), topic);
		const second = foldBookEvent(first, bookEvent(topic, 200), topic);
		expect(second.snapshot?.bids).toEqual([{ price: 200, size: 1 }]);
	});

	test('a venue that serves no depth stays that way — later frames do not argue with it', () => {
		// Asymmetric with `failed` on purpose: "this venue has no depth" is a
		// fact about the venue, "this attempt failed" is a fact about one
		// attempt.
		const unsupported = foldBookEvent(NO_BOOK_YET, unsupportedEvent(topic), topic);
		expect(unsupported.unsupported).toBe(true);
		expect(foldBookEvent(unsupported, bookEvent(topic, 100), topic).unsupported).toBe(true);
	});

	test('another instrument’s frames are not this panel’s', () => {
		const other = 'book:okx-spot:ETH-USDT';
		const state = foldBookEvent(NO_BOOK_YET, bookEvent(other, 100), topic);
		expect(state).toEqual(NO_BOOK_YET);
		expect(foldBookEvent(NO_BOOK_YET, failedEvent(other), topic).failed).toBe(false);
	});

	test('an event about nothing at all leaves what is known untouched', () => {
		const known = foldBookEvent(NO_BOOK_YET, bookEvent(topic, 100), topic);
		expect(foldBookEvent(known, null, topic)).toBe(known);
		expect(foldBookEvent(known, { type: 'price', receivedAt: at, payload: { topic } }, topic)).toBe(known);
	});
});
