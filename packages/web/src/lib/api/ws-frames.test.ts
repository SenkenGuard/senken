import { describe, expect, test } from 'bun:test';
import { isUnsupportedFor, quoteFromEvent, indicatorFrameFor, bookFrameFor } from './ws-frames';
import type { WsEvent } from './ws-events.svelte';

function frame(type: string, payload: unknown): WsEvent {
	return { type, payload, receivedAt: 0 };
}

describe('isUnsupportedFor — the explicit "no feed for this topic" frame', () => {
	test('no event at all is not an unsupported answer', () => {
		expect(isUnsupportedFor(null, 'okx-spot:BTCUSDT')).toBe(false);
	});

	test('an unsupported frame naming this exact topic is recognised', () => {
		const event = frame('unsupported', { topic: 'binance-spot:BTCUSDT' });
		expect(isUnsupportedFor(event, 'binance-spot:BTCUSDT')).toBe(true);
	});

	test('an unsupported frame for a different topic does not match', () => {
		const event = frame('unsupported', { topic: 'binance-spot:ETHUSDT' });
		expect(isUnsupportedFor(event, 'binance-spot:BTCUSDT')).toBe(false);
	});

	// The one thing this check must never confuse: a live price tick is not
	// an absence-of-feed answer, even for the same topic.
	test('a price frame for the same topic is not read as unsupported', () => {
		const event = frame('price', { topic: 'okx-spot:BTCUSDT', price: 12345, price_scale: 2, ts: 0 });
		expect(isUnsupportedFor(event, 'okx-spot:BTCUSDT')).toBe(false);
	});

	test('a malformed payload (no topic field) is refused rather than throwing', () => {
		const event = frame('unsupported', { reason: 'no pool' });
		expect(isUnsupportedFor(event, 'okx-spot:BTCUSDT')).toBe(false);
	});
});

describe('quoteFromEvent', () => {
	test('converts both quote sides from the shared scaled integer precision', () => {
		const quote = frame('quote', {
			topic: 'quote:okx-spot:BTCUSDT',
			bid: 77_995_500,
			ask: 77_995_600,
			price_scale: 3
		});

		expect(quoteFromEvent(quote, 'quote:okx-spot:BTCUSDT')).toEqual({ bid: 77_995.5, ask: 77_995.6 });
	});

	test('refuses a quote for a different instrument', () => {
		const quote = frame('quote', { topic: 'quote:okx-spot:ETHUSDT', bid: 1, ask: 2, price_scale: 0 });

		expect(quoteFromEvent(quote, 'quote:okx-spot:BTCUSDT')).toBeNull();
	});
});

describe('indicatorFrameFor — the live indicator session reading', () => {
	const topic = 'indicator:okx-spot:BTCUSDT:1h:Sma:{"period":20}';

	test('no event at all is not a reading', () => {
		expect(indicatorFrameFor(null, topic)).toBeNull();
	});

	test('an indicator frame for this exact topic is returned whole', () => {
		const event = frame('indicator', { topic, field: 'value', value: 123.45, ts_open: 1_700_000_000_000_000_000, provisional: true });
		expect(indicatorFrameFor(event, topic)).toEqual({
			topic,
			field: 'value',
			value: 123.45,
			ts_open: 1_700_000_000_000_000_000,
			provisional: true
		});
	});

	// The property this jalur's whole point rests on: a chart must be able
	// to tell a still-moving reading apart from a settled one, and this is
	// the one place that distinction actually survives the wire. Losing it
	// (treating every reading as final) is exactly the false-positive shape
	// this project has already had to guard against on the alert side.
	test('distinguishes a provisional reading from a settled one for the same field', () => {
		const provisional = indicatorFrameFor(
			frame('indicator', { topic, field: 'value', value: 100, ts_open: 1, provisional: true }),
			topic
		);
		const settled = indicatorFrameFor(
			frame('indicator', { topic, field: 'value', value: 100, ts_open: 1, provisional: false }),
			topic
		);
		expect(provisional?.provisional).toBe(true);
		expect(settled?.provisional).toBe(false);
	});

	test('a different field under the same topic (a compound indicator) is not confused with another', () => {
		const macdLine = indicatorFrameFor(
			frame('indicator', { topic, field: 'macd_line', value: 1, ts_open: 1, provisional: false }),
			topic
		);
		const macdSignal = indicatorFrameFor(
			frame('indicator', { topic, field: 'macd_signal', value: 2, ts_open: 1, provisional: false }),
			topic
		);
		expect(macdLine?.field).toBe('macd_line');
		expect(macdSignal?.field).toBe('macd_signal');
	});

	test('refuses a reading for a different topic', () => {
		const event = frame('indicator', { topic: 'indicator:okx-spot:ETHUSDT:1h:Sma:{"period":20}', field: 'value', value: 1, ts_open: 1, provisional: false });
		expect(indicatorFrameFor(event, topic)).toBeNull();
	});

	test('a price frame is never read as an indicator reading, even sharing a field name by coincidence', () => {
		const event = frame('price', { topic, price: 1, price_scale: 0, qty: 1, qty_scale: 0, ts: 0 });
		expect(indicatorFrameFor(event, topic)).toBeNull();
	});

	test('a malformed payload (missing provisional) is refused rather than throwing', () => {
		const event = frame('indicator', { topic, field: 'value', value: 1, ts_open: 1 });
		expect(indicatorFrameFor(event, topic)).toBeNull();
	});
});

describe('bookFrameFor — the one-shot order-book depth snapshot', () => {
	const topic = 'book:okx-spot:BTCUSDT';

	test('no event at all is not a snapshot', () => {
		expect(bookFrameFor(null, topic)).toBeNull();
	});

	test('a book frame for this exact topic is returned whole, levels kept as scaled integers', () => {
		const event = frame('book', {
			topic,
			bids: [{ price: 779_274, size: 39_772_851 }],
			asks: [{ price: 779_275, size: 153_603_807 }],
			price_scale: 1,
			qty_scale: 8,
			ts: 1_788_253_213_356
		});
		expect(bookFrameFor(event, topic)).toEqual({
			topic,
			bids: [{ price: 779_274, size: 39_772_851 }],
			asks: [{ price: 779_275, size: 153_603_807 }],
			price_scale: 1,
			qty_scale: 8,
			ts: 1_788_253_213_356
		});
	});

	test('refuses a snapshot for a different topic', () => {
		const event = frame('book', { topic: 'book:okx-spot:ETHUSDT', bids: [], asks: [], price_scale: 1, qty_scale: 8, ts: 0 });
		expect(bookFrameFor(event, topic)).toBeNull();
	});

	test('a quote frame is never read as a book snapshot, even sharing a price_scale field', () => {
		const event = frame('quote', { topic, bid: 1, ask: 2, price_scale: 0 });
		expect(bookFrameFor(event, topic)).toBeNull();
	});

	test('an empty book (both sides genuinely empty) is a valid, distinct snapshot — not a malformed one', () => {
		const event = frame('book', { topic, bids: [], asks: [], price_scale: 1, qty_scale: 8, ts: 0 });
		expect(bookFrameFor(event, topic)).toEqual({ topic, bids: [], asks: [], price_scale: 1, qty_scale: 8, ts: 0 });
	});

	test('a malformed payload (a level missing size) is refused rather than throwing', () => {
		const event = frame('book', { topic, bids: [{ price: 1 }], asks: [], price_scale: 1, qty_scale: 8, ts: 0 });
		expect(bookFrameFor(event, topic)).toBeNull();
	});
});
