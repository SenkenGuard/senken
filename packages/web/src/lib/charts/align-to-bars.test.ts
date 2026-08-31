import { describe, expect, test } from 'bun:test';
import type { UTCTimestamp } from 'lightweight-charts';

import { alignToBarTimes } from './align-to-bars';

/** Ten one-minute bars, the shape a main chart hands down. */
const bars = Array.from({ length: 10 }, (_, i) => (1_700_000_000 + i * 60) as UTCTimestamp);

describe('alignToBarTimes', () => {
	test('puts a warmed-up indicator at the index of the bar it belongs to', () => {
		// An RSI-like warm-up: nothing until the third bar.
		const points = bars.slice(3).map((time, i) => ({ time: time as unknown as number, value: i }));

		const aligned = alignToBarTimes(bars, points);

		// Without the padding this array would start at bar 3's value, so
		// index 0 would carry a reading belonging three bars to the right —
		// which is exactly the shift seen on screen.
		expect(aligned).toHaveLength(bars.length);
		expect(aligned[0]).toEqual({ time: bars[0] });
		expect(aligned[2]).toEqual({ time: bars[2] });
		expect(aligned[3]).toEqual({ time: bars[3], value: 0 });
		expect(aligned[9]).toEqual({ time: bars[9], value: 6 });
	});

	test('keeps the sub-pane exactly as long as the main chart', () => {
		const points = bars.map((time, i) => ({ time: time as unknown as number, value: i }));

		expect(alignToBarTimes(bars.slice(0, 4), points)).toHaveLength(4);
	});

	test('drops a point the main chart is not showing', () => {
		// A replay cut, or a wider compute window than the chart's: the
		// indicator knows about bars that are not on screen.
		const points = [
			{ time: (bars[0] as unknown as number) - 60, value: 99 },
			{ time: bars[1] as unknown as number, value: 1 }
		];

		const aligned = alignToBarTimes(bars.slice(0, 3), points);

		expect(aligned).toEqual([{ time: bars[0] }, { time: bars[1], value: 1 }, { time: bars[2] }]);
	});

	test('holds a gap in the middle open rather than closing it up', () => {
		// A closed-up gap would move every later point one bar left.
		const points = [0, 1, 3].map((i) => ({ time: bars[i] as unknown as number, value: i }));

		const aligned = alignToBarTimes(bars.slice(0, 4), points);

		expect(aligned[2]).toEqual({ time: bars[2] });
		expect(aligned[3]).toEqual({ time: bars[3], value: 3 });
	});
});
