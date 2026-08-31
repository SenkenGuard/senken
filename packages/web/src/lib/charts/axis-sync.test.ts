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

	/** Moves the range the way the library moves it internally when a chart
	 * is handed a new series: the range really is somewhere else, and the
	 * announcement is not something a caller can tell from a gesture. */
	driftTo(range: LogicalRange): void {
		this.range = range;
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

	test('linking a fresh sub-pane never moves the main chart', () => {
		// A sub-pane is rebuilt whenever its layer is re-created, and a fresh
		// chart has no data. Linking hands it the main chart's range, and if
		// that hand-off is not guarded the sub echoes straight back —
		// whatever an empty chart made of the range becomes the main chart's
		// scroll position. On screen that reads as the price chart jumping
		// so its last bar sits at the far left.
		const main = new FakeChart();
		main.timeScale().setVisibleLogicalRange(logicalRange(400, 500));
		const mainScale = main.timeScale();
		mainScale.setCalls = 0;

		linkTimeScales(main, new FakeChart());

		expect(mainScale.setCalls).toBe(0);
	});

	test('a sub-pane receiving data never moves the main chart', () => {
		const main = new FakeChart();
		const sub = new FakeChart();
		const link = linkTimeScales(main, sub);
		main.timeScale().setVisibleLogicalRange(logicalRange(400, 500));
		const mainScale = main.timeScale();
		mainScale.setCalls = 0;

		// The library re-anchors a chart when it is handed a new series, and
		// announces it from *inside* the data call. Suppressing only after
		// that call returns is too late: the leader has already moved.
		link.applyToFollower(() => {
			sub.timeScale().setVisibleLogicalRange(logicalRange(0, 10));
		});

		expect(mainScale.setCalls).toBe(0);
		expect(main.timeScale().getVisibleLogicalRange()).toEqual(logicalRange(400, 500));
		// And the strip ends up back under the price chart, not where the
		// library left it.
		expect(sub.timeScale().getVisibleLogicalRange()).toEqual(logicalRange(400, 500));
	});

	test('a real gesture on a sub-pane still leads, once the update is over', () => {
		// Suppression must not outlive the data update, or panning an
		// indicator strip would stop moving the pane.
		const main = new FakeChart();
		const sub = new FakeChart();
		const link = linkTimeScales(main, sub);
		link.applyToFollower(() => {
			sub.timeScale().setVisibleLogicalRange(logicalRange(0, 10));
		});

		sub.timeScale().setVisibleLogicalRange(logicalRange(7, 9));

		expect(main.timeScale().getVisibleLogicalRange()).toEqual(logicalRange(7, 9));
	});

	test('the disposer stops propagation in both directions', () => {
		const main = new FakeChart();
		const sub = new FakeChart();
		const link = linkTimeScales(main, sub);

		link.dispose();
		main.timeScale().setVisibleLogicalRange(logicalRange(0, 100));
		expect(sub.timeScale().getVisibleLogicalRange()).toBeNull();

		sub.timeScale().setVisibleLogicalRange(logicalRange(7, 9));
		expect(main.timeScale().getVisibleLogicalRange()).toEqual(logicalRange(0, 100));
	});
});
