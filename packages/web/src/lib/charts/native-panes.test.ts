import { describe, expect, test } from 'bun:test';
import type { Logical, LogicalRange } from 'lightweight-charts';
import { nativePaneData } from './native-panes';
import { preserveVisibleRange } from './chart-window';

function range(from: number, to: number): LogicalRange {
	return { from: from as Logical, to: to as Logical };
}

describe('native chart panes', () => {
	test('a warmed-up indicator still ends on the main pane last bar', () => {
		const barTimes = [10, 20, 30, 40, 50];
		const indicator = nativePaneData(
			barTimes.slice(2).map((time) => ({ time, value: time * 2 })),
			2
		);

		expect(Number(indicator.at(-1)?.time)).toBe(50);
		expect(Number(indicator[0]?.time)).toBe(30);
	});

	test('adding and removing a drawing does not shift the visible range', () => {
		let visible = range(400, 500);
		const drawings: string[] = [];
		preserveVisibleRange(
			{
				getVisibleLogicalRange: () => visible,
				setVisibleLogicalRange: (next) => (visible = next)
			},
			() => {
				drawings.push('line');
				drawings.pop();
				visible = range(0, 10);
			}
		);

		expect(visible).toEqual(range(400, 500));
	});

	test('the first native sub-pane load preserves the main range', () => {
		let visible = range(100, 200);
		let loaded = false;
		preserveVisibleRange(
			{
				getVisibleLogicalRange: () => visible,
				setVisibleLogicalRange: (next) => (visible = next)
			},
			() => {
				loaded = nativePaneData([{ time: 200, value: 7 }], 1).length === 1;
				visible = range(0, 1);
			}
		);

		expect(loaded).toBe(true);
		expect(visible).toEqual(range(100, 200));
	});
});
