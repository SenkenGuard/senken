// The pure half of the chart position/order lines feature — the "which
// lines should exist" decision, which is also the half a stale-store bug
// would show up in. The drawing half is an `$effect` over `lightweight-charts`
// and is not exercised here; see `chart-pane.svelte`'s own price-line effect
// and this repo's report for that plan for why it can't be.
import { describe, expect, test } from 'bun:test';
import type { OrderDto, PositionDto } from '$lib/api/types';
import { tradeLinesFor } from './trade-lines';

function position(overrides: Partial<PositionDto> = {}): PositionDto {
	return {
		account_id: 'acct-1',
		instrument: 'okx-spot:BTCUSDT',
		side: 'long',
		quantity: { scale: 2, value: '25' },
		average_entry: { scale: 2, value: '6842013' },
		mark_price: null,
		unrealized_pnl: null,
		realized_pnl: { scale: 2, value: '0' },
		margin: null,
		leverage: null,
		opened_at: 0,
		...overrides
	};
}

function order(overrides: Partial<OrderDto> = {}): OrderDto {
	return {
		id: 'o1',
		client_order_id: null,
		account_id: 'acct-1',
		instrument: 'okx-spot:BTCUSDT',
		side: 'buy',
		kind: 'limit',
		limit_price: { scale: 2, value: '6700000' },
		trigger_price: null,
		quantity: { scale: 2, value: '25' },
		filled_quantity: { scale: 2, value: '0' },
		average_price: null,
		time_in_force: 'gtc',
		status: 'open',
		reduce_only: false,
		submitted_at: 0,
		updated_at: 0,
		reject_reason: null,
		...overrides
	};
}

describe('tradeLinesFor — instrument scoping', () => {
	test('only rows on the pane\'s own instrument produce lines', () => {
		const lines = tradeLinesFor(
			'okx-spot:BTCUSDT',
			[position({ instrument: 'okx-spot:BTCUSDT' }), position({ instrument: 'okx-spot:ETHUSDT' })],
			[order({ instrument: 'okx-spot:BTCUSDT' }), order({ id: 'o2', instrument: 'okx-spot:ETHUSDT' })],
			2
		);
		// Exactly one position line and one order line — the ETHUSDT rows must
		// not leak onto this pane.
		expect(lines.filter((l) => l.kind === 'position')).toHaveLength(1);
		expect(lines.filter((l) => l.kind === 'order')).toHaveLength(1);
		expect(lines.every((l) => l.key.includes('acct-1'))).toBe(true);
	});

	test('a pane with no rows on its instrument draws nothing, even with rows elsewhere', () => {
		const lines = tradeLinesFor(
			'okx-spot:SOLUSDT',
			[position({ instrument: 'okx-spot:BTCUSDT' })],
			[order({ instrument: 'okx-spot:BTCUSDT' })],
			2
		);
		expect(lines).toHaveLength(0);
	});
});

describe('tradeLinesFor — order working state', () => {
	test('a filled order produces no line', () => {
		const lines = tradeLinesFor('okx-spot:BTCUSDT', [], [order({ status: 'filled' })], 2);
		expect(lines).toHaveLength(0);
	});

	test('a cancelled order produces no line', () => {
		const lines = tradeLinesFor('okx-spot:BTCUSDT', [], [order({ status: 'cancelled' })], 2);
		expect(lines).toHaveLength(0);
	});

	test('a still-working order (open, pending, partially_filled) produces a line', () => {
		for (const status of ['open', 'pending', 'partially_filled']) {
			const lines = tradeLinesFor('okx-spot:BTCUSDT', [], [order({ status })], 2);
			expect(lines).toHaveLength(1);
		}
	});
});

describe('tradeLinesFor — which price an order line sits at', () => {
	test('a limit order uses its limit price', () => {
		const [line] = tradeLinesFor(
			'okx-spot:BTCUSDT',
			[],
			[order({ kind: 'limit', limit_price: { scale: 2, value: '6700000' }, trigger_price: null })],
			2
		);
		expect(line.price).toBe(67000);
	});

	test('a stop order with no limit uses its trigger price', () => {
		const [line] = tradeLinesFor(
			'okx-spot:BTCUSDT',
			[],
			[order({ kind: 'stop', limit_price: null, trigger_price: { scale: 2, value: '6900000' } })],
			2
		);
		expect(line.price).toBe(69000);
	});

	test('a working order with neither price is skipped rather than drawn at a made-up level', () => {
		const lines = tradeLinesFor(
			'okx-spot:BTCUSDT',
			[],
			[order({ kind: 'market', limit_price: null, trigger_price: null })],
			2
		);
		expect(lines).toHaveLength(0);
	});
});

describe('tradeLinesFor — tone', () => {
	test('a long position is gain-toned', () => {
		const [line] = tradeLinesFor('okx-spot:BTCUSDT', [position({ side: 'long' })], [], 2);
		expect(line.tone).toBe('gain');
	});

	test('a short position is loss-toned', () => {
		const [line] = tradeLinesFor('okx-spot:BTCUSDT', [position({ side: 'short' })], [], 2);
		expect(line.tone).toBe('loss');
	});

	test('an order is always neutral-toned, on either side', () => {
		const buy = tradeLinesFor('okx-spot:BTCUSDT', [], [order({ side: 'buy' })], 2)[0];
		const sell = tradeLinesFor('okx-spot:BTCUSDT', [], [order({ side: 'sell', id: 'o2' })], 2)[0];
		expect(buy.tone).toBe('neutral');
		expect(sell.tone).toBe('neutral');
	});
});

describe('tradeLinesFor — the scaled-to-number conversion is exact', () => {
	test('at scale 2 (the simulator\'s cash scale)', () => {
		const [line] = tradeLinesFor(
			'okx-spot:BTCUSDT',
			[position({ average_entry: { scale: 2, value: '6842013' } })],
			[],
			2
		);
		expect(line.price).toBe(68420.13);
	});

	test('at scale 8 (a crypto instrument tick), including a value a naive parse would round', () => {
		// `formatScaledForDisplay` — the display helper the rest of the trade
		// UI reaches for — trims to 2 fractional digits by default. Reusing it
		// here would silently turn 6.84201234 into 6.84, which is exactly the
		// naive mistake this conversion has to avoid: the drawn line would sit
		// visibly off the real entry price.
		const [line] = tradeLinesFor(
			'okx-spot:BTCUSDT',
			[position({ average_entry: { scale: 8, value: '684201234' } })],
			[],
			8
		);
		expect(line.price).toBe(6.84201234);
	});
});

describe('tradeLinesFor — key stability', () => {
	test('two calls with equal input produce the same keys in the same order, so a redraw diffs to nothing', () => {
		const positions = [position()];
		const orders = [order()];
		const first = tradeLinesFor('okx-spot:BTCUSDT', positions, orders, 2);
		const second = tradeLinesFor('okx-spot:BTCUSDT', positions, orders, 2);
		expect(second.map((l) => l.key)).toEqual(first.map((l) => l.key));
	});

	test('two distinct rows never collide on the same key', () => {
		const lines = tradeLinesFor(
			'okx-spot:BTCUSDT',
			[position({ account_id: 'acct-1' }), position({ account_id: 'acct-2' })],
			[order({ id: 'o1' }), order({ id: 'o2' })],
			2
		);
		const keys = lines.map((l) => l.key);
		expect(new Set(keys).size).toBe(keys.length);
	});
});
