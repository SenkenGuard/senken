import { describe, expect, test } from 'bun:test';
import { linkTimeScales, type ChartLike, type TimeScaleLike } from './axis-sync';
import type { Logical, LogicalRange, LogicalRangeChangeEventHandler } from 'lightweight-charts';

/** `Logical` is a nominally-branded number (its own `[Symbol.species]` tag),
 * so a plain `logicalRange(5, 25)` literal does not satisfy `LogicalRange`
 * without a cast — this test only ever needs plain bar-index arithmetic, not
 * the brand itself. */
function logicalRange(from: number, to: number): LogicalRange {
	return { from: from as Logical, to: to as Logical };
}

/** A minimal stand-in for `lightweight-charts`' own time scale: setting a
 * range synchronously notifies every subscriber with the new range, the
 * same way the real library's `setVisibleLogicalRange` does — that
 * synchronous re-entrancy is exactly what `linkTimeScales`' guard exists to
 * survive. `setCalls` counts every call to `setVisibleLogicalRange`,
 * however it was triggered, so a test can tell a single user gesture from
 * an echoed one. */
class FakeTimeScale implements TimeScaleLike {
	private range: LogicalRange | null = null;
	private listeners: LogicalRangeChangeEventHandler[] = [];
	setCalls = 0;

	getVisibleLogicalRange(): LogicalRange | null {
		return this.range;
	}

	setVisibleLogicalRange(range: LogicalRange): void {
		this.setCalls++;
		this.range = range;
		for (const listener of this.listeners) listener(range);
	}

	subscribeVisibleLogicalRangeChange(handler: LogicalRangeChangeEventHandler): void {
		this.listeners.push(handler);
	}

	unsubscribeVisibleLogicalRangeChange(handler: LogicalRangeChangeEventHandler): void {
		this.listeners = this.listeners.filter((l) => l !== handler);
	}
}

class FakeChart implements ChartLike {
	private scale = new FakeTimeScale();
	timeScale(): FakeTimeScale {
		return this.scale;
	}
}

describe('linkTimeScales — main pane / sub-pane x-axis sync', () => {
	test('the leader propagates its range to every follower', () => {
		const main = new FakeChart();
		const sub1 = new FakeChart();
		const sub2 = new FakeChart();
		linkTimeScales(main, sub1);
		linkTimeScales(main, sub2);

		main.timeScale().setVisibleLogicalRange(logicalRange(5, 25));

		expect(sub1.timeScale().getVisibleLogicalRange()).toEqual(logicalRange(5, 25));
		expect(sub2.timeScale().getVisibleLogicalRange()).toEqual(logicalRange(5, 25));
	});

	test('dragging a sub-pane directly still moves the main pane and its sibling sub-pane', () => {
		const main = new FakeChart();
		const sub1 = new FakeChart();
		const sub2 = new FakeChart();
		linkTimeScales(main, sub1);
		linkTimeScales(main, sub2);

		// A user can scroll/zoom a sub-pane's own axis directly, not just the
		// main one — the hub is whichever chart the gesture actually touches.
		sub1.timeScale().setVisibleLogicalRange(logicalRange(1, 2));

		expect(main.timeScale().getVisibleLogicalRange()).toEqual(logicalRange(1, 2));
		expect(sub2.timeScale().getVisibleLogicalRange()).toEqual(logicalRange(1, 2));
	});

	test("re-entry guard: a follower's mirrored update does not bounce back and oscillate", () => {
		const main = new FakeChart();
		const sub1 = new FakeChart();
		const sub2 = new FakeChart();
		linkTimeScales(main, sub1);
		linkTimeScales(main, sub2);

		const mainScale = main.timeScale();
		const sub1Scale = sub1.timeScale();
		const sub2Scale = sub2.timeScale();
		mainScale.setCalls = 0;
		sub1Scale.setCalls = 0;
		sub2Scale.setCalls = 0;

		// One real gesture on the leader.
		mainScale.setVisibleLogicalRange(logicalRange(10, 20));

		// Without the guard, sub1's mirrored change would fire its own
		// subscribeVisibleLogicalRangeChange handler, which would set the
		// range back on `main`, which would fire again, and so on — this
		// is exactly the "looks like the chart is stuttering" oscillation
		// the guard exists to prevent. Each chart's range is set exactly
		// once per gesture, never echoed.
		expect(mainScale.setCalls).toBe(1);
		expect(sub1Scale.setCalls).toBe(1);
		expect(sub2Scale.setCalls).toBe(1);
	});

	test('the disposer stops propagation in both directions', () => {
		const main = new FakeChart();
		const sub = new FakeChart();
		const unlink = linkTimeScales(main, sub);

		unlink();
		main.timeScale().setVisibleLogicalRange(logicalRange(0, 100));
		expect(sub.timeScale().getVisibleLogicalRange()).toBeNull();

		sub.timeScale().setVisibleLogicalRange(logicalRange(7, 9));
		expect(main.timeScale().getVisibleLogicalRange()).toEqual(logicalRange(0, 100));
	});
});
