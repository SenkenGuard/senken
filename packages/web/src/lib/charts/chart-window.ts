import type { LogicalRange } from 'lightweight-charts';
import { defaultBarWindow } from '$lib/components/terminal/chart-config';

/** Number of bars in one history request. Keeping this equal to the initial
 * window makes a left-edge request predictable and avoids a second, smaller
 * policy hidden in the renderer. */
export const HISTORY_PAGE_BARS = 300;

/** A pane keeps at most this many contiguous bars in memory.
 *
 * This is a memory backstop, not a viewing limit, and the difference matters:
 * at 20,000 it was the latter. Twenty thousand one-minute bars is thirteen
 * days — a chart that stops there is not bounded, it is broken. A hundred
 * thousand is seventy days of M1 and eleven years of H1, which is past what a
 * reader explores in one sitting and still far below the point where one pane
 * dominates the heap. */
export const MAX_HISTORY_BARS = 100_000;

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

/** The pane's left-edge status, shown to the reader as three visibly
 * different states rather than one ambiguous blank: a page in flight, the
 * venue's own history genuinely exhausted, or a fetch that failed. `idle`
 * renders nothing — it is not a fourth visible state, it is the absence of
 * one. */
export type HistoryEdgeState = 'idle' | 'loading' | 'end' | 'error' | 'full';

/** A range the server reported as genuinely missing from the venue
 * (`BarRangeResponse.missing`) — not merely unfetched. All fields are Unix
 * nanos. */
export interface HistoryGap {
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

/** The window a date-jump control actually requests, honest about the three
 * ways a jump can land relative to what the venue has (B11a):
 *
 * - Past the newest bar: there is nothing to centre on, so this resolves to
 *   the same trailing window the pane opens with — the reader lands where
 *   "now" already is, not on empty future space.
 * - Before the venue's known earliest bar: centring would request a window
 *   whose left half can never be filled, so this anchors the window's start
 *   at the known edge instead and lets the caller mark it final.
 * - Anywhere else: centred on the requested instant, unclamped.
 *
 * `earliestAvailable` is `null` until a previous fetch has actually observed
 * the edge — an unknown edge does not clamp, since a short first response
 * from the resulting fetch reports the true edge just as accurately. */
export function resolveJumpWindow(
	target: number,
	barWidth: number,
	nowNanos: number,
	earliestAvailable: number | null
): HistoryWindow {
	const width = barWidth * HISTORY_PAGE_BARS;
	if (target >= nowNanos) return { from: nowNanos - width, to: nowNanos };
	const centred = historyWindowAround(target, barWidth);
	if (earliestAvailable != null && centred.from < earliestAvailable) {
		return { from: earliestAvailable, to: earliestAvailable + width };
	}
	return centred;
}

/** The bars a pane holds after a date jump resolves. A jump **replaces** the
 * previously loaded window rather than folding into it — appending would
 * place two time-disconnected islands in one array, which the renderer and
 * every indicator downstream of it would then draw as a single honest
 * range. `current` is intentionally unused: the invariant this exists to
 * hold is that nothing from the old window survives a jump. */
export function applyHistoryJump<T>(current: readonly T[], resolved: readonly T[]): T[] {
	void current;
	return [...resolved];
}

/** Whether a visible logical range is near enough to the left edge to load
 * the predecessor page. `null` is what lightweight-charts reports before it
 * has data; it is never a request signal. */
export function shouldPrefetchHistory(visibleFrom: number | null, loadedBars: number): boolean {
	return visibleFrom != null && visibleFrom <= HISTORY_PREFETCH_THRESHOLD_BARS && loadedBars > 0;
}

/** Whether a pane is holding all the history it is allowed to.
 *
 * Checked *before* the request, not after: trimming after a prepend is what
 * turned the cap into a wall, because it dropped the oldest bars — the page
 * that had just been fetched. Every drag past the cap then spent venue
 * requests on bars it threw away immediately.
 *
 * Trimming the *newest* instead would keep paging alive but leave a hole at
 * the right edge that nothing refetches, so scrolling back toward now would
 * silently show nothing. Refusing is the only answer with no hole in it, and
 * jumping to a date is the way further back: it replaces the window rather
 * than extending it, so the pane stays one contiguous range. */
export function historyIsFull(loadedBars: number): boolean {
	return loadedBars >= MAX_HISTORY_BARS;
}

/** Runs a structural renderer update without moving what the reader is
 * looking at.
 *
 * Restores the visible range **by time**, not by logical index, and the
 * difference is the whole point. Prepending a page of history renumbers every
 * bar: the bar that was logical 0 becomes logical 300. Capturing and putting
 * back the same logical numbers therefore leaves the viewport pointing at
 * three hundred different bars — the chart lurches backwards the instant a
 * page lands, which is exactly what paging felt like. A time range means the
 * same instants before and after, so the bars under the reader's eyes stay
 * under them.
 *
 * Falls back to the logical range only when the series has no time range yet
 * (an empty chart), where there is nothing to hold still anyway. */
export function preserveVisibleRange<T>(
	timeScale: {
		getVisibleRange(): { from: T; to: T } | null;
		setVisibleRange(range: { from: T; to: T }): void;
		getVisibleLogicalRange(): LogicalRange | null;
		setVisibleLogicalRange(range: LogicalRange): void;
	},
	update: () => void
): void {
	const times = timeScale.getVisibleRange();
	const logical = times ? null : timeScale.getVisibleLogicalRange();
	update();
	if (times) timeScale.setVisibleRange(times);
	else if (logical) timeScale.setVisibleLogicalRange(logical);
}

/** The range every indicator path must cover: exactly the bars a pane is
 * holding, never the window it first asked for.
 *
 * Paging older history prepends to the loaded bars and jumping to a date
 * replaces them, so a range derived from the initial window stays frozen at
 * its original left edge — candles extend back while the plots stop dead.
 * `null` for an empty pane, which has nothing to compute over yet. */
export function indicatorRange(
	bars: readonly { ts_open: number }[],
	stepSeconds: number
): { from: number; to: number } | null {
	const first = bars[0];
	const last = bars[bars.length - 1];
	if (!first || !last) return null;
	return { from: first.ts_open, to: last.ts_open + stepSeconds * 1_000_000_000 };
}
