import { describe, expect, test } from 'bun:test';
import { TopicRefCounter } from './topic-refcount';

describe('TopicRefCounter', () => {
	test('the first retain on a topic reports true; later ones report false', () => {
		const counter = new TopicRefCounter();
		expect(counter.retain('okx-spot:BTCUSDT')).toBe(true);
		expect(counter.retain('okx-spot:BTCUSDT')).toBe(false);
		expect(counter.retain('okx-spot:BTCUSDT')).toBe(false);
	});

	test('a topic survives one release while another reference is still outstanding', () => {
		// The exact scenario this class exists for: a chart pane and a
		// watchlist row both leasing the same instrument. The pane closing
		// (one release) must not tear down the watchlist row's own lease.
		const counter = new TopicRefCounter();
		counter.retain('okx-spot:BTCUSDT'); // chart pane
		counter.retain('okx-spot:BTCUSDT'); // watchlist row

		expect(counter.release('okx-spot:BTCUSDT')).toBe(false);
		expect(Array.from(counter.topics())).toEqual(['okx-spot:BTCUSDT']);

		// Only the second, matching release actually drops it.
		expect(counter.release('okx-spot:BTCUSDT')).toBe(true);
		expect(Array.from(counter.topics())).toEqual([]);
	});

	test('releasing a topic with no outstanding reference is a safe no-op', () => {
		const counter = new TopicRefCounter();
		expect(counter.release('okx-spot:BTCUSDT')).toBe(false);

		counter.retain('okx-spot:BTCUSDT');
		counter.release('okx-spot:BTCUSDT');
		// Already back to zero — a second release must not go negative or
		// report "last reference released" again.
		expect(counter.release('okx-spot:BTCUSDT')).toBe(false);
	});

	test('topics() lists only currently-referenced topics, for reconnect replay', () => {
		const counter = new TopicRefCounter();
		counter.retain('okx-spot:BTCUSDT');
		counter.retain('okx-spot:ETHUSDT');
		counter.release('okx-spot:ETHUSDT');

		expect(Array.from(counter.topics())).toEqual(['okx-spot:BTCUSDT']);
	});
});
