import { describe, expect, test } from 'bun:test';
import type { Logical, LogicalRange } from 'lightweight-charts';
import { nativePaneData } from './native-panes';
import { preserveVisibleRange } from './chart-window';

function range(from: number, to: number): LogicalRange {
	return { from: from as Logical, to: to as Logical };
}

/** A time scale over a real series: logical indices address positions in
 * `times`, so prepending renumbers them exactly as lightweight-charts does. */
function scaleOver(initial: number[]) {
	let times = [...initial];
	let logical = range(0, initial.length - 1);
	return {
		prepend(older: number[]) {
			times = [...older, ...times];
		},
		visibleTimes(): [number | undefined, number | undefined] {
			return [times[Math.round(logical.from)], times[Math.round(logical.to)]];
		},
		getVisibleRange: () => {
			const from = times[Math.round(logical.from)];
			const to = times[Math.round(logical.to)];
			return from == null || to == null ? null : { from, to };
		},
		setVisibleRange: (next: { from: number; to: number }) => {
			logical = range(times.indexOf(next.from), times.indexOf(next.to));
		},
		getVisibleLogicalRange: () => logical,
		setVisibleLogicalRange: (next: LogicalRange) => (logical = next)
	};
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

	// The one that paging depends on. A page of older bars renumbers every
	// logical index, so putting the same logical range back leaves the
	// viewport on different bars entirely — the chart lurches backwards the
	// moment a page lands. Holding the *times* is what makes it seamless.
	test('a prepended page of history leaves the reader looking at the same bars', () => {
		const scale = scaleOver([30, 40, 50]);
		expect(scale.visibleTimes()).toEqual([30, 50]);

		preserveVisibleRange(scale, () => scale.prepend([10, 20]));

		expect(scale.visibleTimes()).toEqual([30, 50]);
	});

	test('adding and removing a drawing does not shift the visible range', () => {
		const scale = scaleOver([100, 200, 300, 400, 500]);
		const drawings: string[] = [];

		preserveVisibleRange(scale, () => {
			drawings.push('line');
			drawings.pop();
			scale.setVisibleLogicalRange(range(0, 1));
		});

		expect(scale.visibleTimes()).toEqual([100, 500]);
	});

	test('the first native sub-pane load preserves the main range', () => {
		const scale = scaleOver([100, 200, 300]);
		let loaded = false;

		preserveVisibleRange(scale, () => {
			loaded = nativePaneData([{ time: 200, value: 7 }], 1).length === 1;
			scale.setVisibleLogicalRange(range(0, 1));
		});

		expect(loaded).toBe(true);
		expect(scale.visibleTimes()).toEqual([100, 300]);
	});

	test('an empty chart falls back to the logical range, having no times to hold', () => {
		const scale = scaleOver([]);
		scale.setVisibleLogicalRange(range(0, 10));

		preserveVisibleRange(scale, () => scale.setVisibleLogicalRange(range(99, 100)));

		expect(scale.getVisibleLogicalRange()).toEqual(range(0, 10));
	});
});
