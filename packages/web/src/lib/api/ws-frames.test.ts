import { describe, expect, test } from 'bun:test';
import { isUnsupportedFor } from './ws-frames';
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
