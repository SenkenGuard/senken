import { describe, expect, test } from 'bun:test';
import type { WsBookLevel } from '$lib/api/ws-frames';
import { bookRowsFromSnapshot } from './order-book';

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
