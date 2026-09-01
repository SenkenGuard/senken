import type { LogicalRange } from 'lightweight-charts';
import { defaultBarWindow } from '$lib/components/terminal/chart-config';

/** Number of bars in one history request. Keeping this equal to the initial
 * window makes a left-edge request predictable and avoids a second, smaller
 * policy hidden in the renderer. */
export const HISTORY_PAGE_BARS = 300;

/** A pane keeps at most this many contiguous bars in memory. It is high
 * enough for a reader to explore a long session while bounded well below the
 * point where a single chart dominates the browser heap. */
export const MAX_HISTORY_BARS = 20_000;

/** Do not request another page until the reader is this close to the loaded
 * left edge. This lower threshold than the trim threshold is the hysteresis
 * that prevents a small back-and-forth drag from repeatedly fetching and
 * evicting the same page. */
export const HISTORY_PREFETCH_THRESHOLD_BARS = 80;

/** The request class for history that is outside the viewport. Kept as a
 * named policy so a test can prove it never competes with visible bars. */
export function historyLoadPriority(): 'prefetch' {
	return 'prefetch';
}

/** A contiguous bar window held by one pane. All fields are Unix nanos. */
export interface HistoryWindow {
	from: number;
	to: number;
}

/** Resolves the initial bar request without coupling renderer code to window policy. */
export function initialBarWindow(spec: string): { from: number; to: number } {
	return defaultBarWindow(spec, HISTORY_PAGE_BARS);
}

/** Returns the contiguous predecessor page. It deliberately replaces no
 * part of the current range: callers append this immediately to `window`,
 * whereas a date jump uses `windowAround` and replaces the old range. */
export function previousHistoryPage(window: HistoryWindow): HistoryWindow {
	const width = window.to - window.from;
	return { from: window.from - width, to: window.from };
}

/** A fresh contiguous window centred on a requested instant. A date jump
 * must use this rather than merging with the current window, because a chart
 * series cannot honestly draw two disconnected islands as one line. */
export function historyWindowAround(target: number, barWidth: number): HistoryWindow {
	const width = barWidth * HISTORY_PAGE_BARS;
	const from = target - Math.floor(width / 2);
	return { from, to: from + width };
}

/** Whether a visible logical range is near enough to the left edge to load
 * the predecessor page. `null` is what lightweight-charts reports before it
 * has data; it is never a request signal. */
export function shouldPrefetchHistory(visibleFrom: number | null, loadedBars: number): boolean {
	return visibleFrom != null && visibleFrom <= HISTORY_PREFETCH_THRESHOLD_BARS && loadedBars > 0;
}

/** Drops only complete oldest bars after a contiguous append. The caller
 * keeps the newest `MAX_HISTORY_BARS`, so the result remains exactly one
 * range and cannot turn a later indicator computation into two islands. */
export function retainedHistoryStart(totalBars: number): number {
	return Math.max(0, totalBars - MAX_HISTORY_BARS);
}

/** Runs a structural renderer update without changing the reader's viewport. */
export function preserveVisibleRange(
	timeScale: {
		getVisibleLogicalRange(): LogicalRange | null;
		setVisibleLogicalRange(range: LogicalRange): void;
	},
	update: () => void
): void {
	const range = timeScale.getVisibleLogicalRange();
	update();
	if (range) timeScale.setVisibleLogicalRange(range);
}
