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
 * ways a jump can land relative to what the venue has:
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

/** How far every logical index moved when a pane's bar array was replaced.
 *
 * Prepending a page of history renumbers every bar: the one that was logical
 * 0 becomes logical 300. Putting the same logical numbers back after such an
 * update therefore leaves the viewport pointing at three hundred different
 * bars, and the chart lurches backwards the instant a page lands.
 *
 * Anchored on the first bar the previous window held, falling back to its
 * last — a page prepended in front of it moves the first anchor, a bar
 * appended after it moves neither, and only a window that shares no bar at
 * all with its predecessor (a date jump, a new instrument) has no anchor,
 * which is reported as `null`: there is nothing to hold still, and the
 * caller should frame the new window instead of pretending to preserve
 * something.
 *
 * Both arrays are the pane's own bar times in ascending order, so the anchor
 * is found by binary search rather than a scan — this runs on every re-set of
 * the series, against a window that may hold tens of thousands of bars.
 *
 * One limit, stated rather than hidden: the chart numbers the *union* of
 * every series' time points, so on a pane carrying an overlay instrument
 * whose bars do not line up with the main one's, a prepended page can move
 * the union by more than it moves this series. The frame then drifts by that
 * difference — bounded by the overlay's own missing bars, and only while
 * paging, where it used to lurch by a whole page. */
export function logicalRangeShift(
	previous: readonly number[],
	next: readonly number[]
): number | null {
	if (previous.length === 0 || next.length === 0) return null;
	const firstIndex = indexOfTime(next, previous[0]);
	if (firstIndex >= 0) return firstIndex;
	const lastIndex = indexOfTime(next, previous[previous.length - 1]);
	if (lastIndex >= 0) return lastIndex - (previous.length - 1);
	return null;
}

/** The index of `time` in an ascending array of bar times, or `-1`.
 *
 * Exported because the pane needs the same answer on every crosshair move —
 * to find the bar under the pointer, and the one before it, in a window that
 * may hold tens of thousands of bars. A scan there is a scan per mouse
 * move. */
export function indexOfTime(times: readonly number[], time: number): number {
	let low = 0;
	let high = times.length - 1;
	while (low <= high) {
		const mid = Math.floor((low + high) / 2);
		const value = times[mid];
		if (value === time) return mid;
		if (value < time) low = mid + 1;
		else high = mid - 1;
	}
	return -1;
}

/** Runs a structural update without moving what the reader is looking at.
 *
 * `pending` is the range a caller has just asked the chart for and the chart
 * has not processed yet, and passing it is not an optimisation — it is the
 * whole correctness of this function.
 *
 * A chart applies a range change on its own render tick, not on the call. So
 * a series update that lands in between reads the range as it was *before*
 * that request and writes it straight back, discarding it. Nothing about the
 * two operations looks wrong on its own; together they undo each other, and
 * only when a second series exists to do the reading — which is exactly why a
 * bare chart framed correctly and the same chart with one indicator did not.
 *
 * Measured, not reasoned: framing then updating an indicator collapsed a
 * 306-bar frame back to the 99 bars that preceded it; supplying the pending
 * frame here held all 306. */
export function preserveVisibleRange(
	timeScale: {
		getVisibleLogicalRange(): LogicalRange | null;
		setVisibleLogicalRange(range: LogicalRange): void;
	},
	update: () => void,
	shift: number | null = 0,
	pending: LogicalRange | null = null
): void {
	const logical = shift === null && pending === null ? null : (pending ?? timeScale.getVisibleLogicalRange());
	update();
	if (logical === null) return;
	// A pending frame is already where the reader should end up, so it is
	// re-asserted as-is; only a range read from the chart needs the shift a
	// prepended page introduced.
	const moved = pending ? 0 : (shift ?? 0);
	timeScale.setVisibleLogicalRange(
		moved === 0
			? logical
			: ({ from: logical.from + moved, to: logical.to + moved } as unknown as LogicalRange)
	);
}


/** The frame a pane opens on, and the one "reset chart view" returns to,
 * expressed as a range of bars rather than a zoom level.
 *
 * The previous shape computed `barSpacing` itself — plot width divided by the
 * bars to show — and that arithmetic has two inputs the caller can get wrong
 * at exactly the moment framing happens. A pixel width measured before the
 * pane has been laid out gives a spacing far too small, and the chart opens
 * zoomed out over hundreds of bars; a bar count read from a cache the data
 * effect has not written yet gives a spacing far too large, and one candle
 * fills the pane. Both were reported, and they are the same mistake pointing
 * in opposite directions.
 *
 * A logical range has neither input. It says *which bars* to show and lets
 * the chart derive the spacing from its own width, at its own layout time —
 * so the frame cannot be wrong about a width nobody has measured yet.
 *
 * `to` deliberately runs past the newest bar: that gap is the right-hand
 * margin, and expressing it here rather than as a second `scrollToPosition`
 * call keeps the whole frame one atomic statement. */
export function defaultFrame(
	barCount: number,
	rightOffset: number,
	visibleBars: number = HISTORY_PAGE_BARS
): LogicalRange | null {
	if (barCount <= 0) return null;
	const span = Math.max(1, Math.min(visibleBars, barCount));
	const offset = Math.max(0, rightOffset);
	const to = barCount - 1 + offset;
	return { from: (to - (span - 1) - offset) as LogicalRange['from'], to: to as LogicalRange['to'] };
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

/** How long a frame the chart has not acknowledged may still be treated as
 * pending. The race it exists for is one render tick; a second is generous
 * for that and short enough that a frame the chart silently declined cannot
 * hold the viewport hostage. */
export const PENDING_FRAME_TTL_MS = 1_000;

/** Whether a frame requested at `setAt` is still worth re-asserting.
 *
 * A pending frame is only ever cleared when the chart *reports* the range, and
 * it does not report one it never applied — a frame identical to what is
 * already shown, or one it declined. Without an expiry that leaves the frame
 * pending for the rest of the session, and every later series update
 * re-asserts it: the viewport stops responding at all. That is worse than the
 * race this mechanism exists to close, and it happens with no indicator in
 * sight. */
export function pendingFrameIsLive(setAt: number, now: number): boolean {
	return now - setAt <= PENDING_FRAME_TTL_MS;
}

/** Whether a requested frame differs from what the chart already shows.
 *
 * Nothing is pending when it does not: the chart has no change to apply, so it
 * will never report one, so nothing would ever clear it. */
export function frameIsPending(current: LogicalRange | null, frame: LogicalRange): boolean {
	if (!current) return true;
	return Number(current.from) !== Number(frame.from) || Number(current.to) !== Number(frame.to);
}

/** The window a reload should ask for: everything the pane is already
 * holding, brought up to now.
 *
 * A reload is not a fresh open. The bars effect re-runs whenever a bar closes,
 * and asking for the initial window again throws away every page of history
 * the reader had scrolled in — the window snaps from six hundred bars back to
 * three hundred, which drags the viewport with it, which puts the left edge
 * back in range of the prefetch, which pages the same three hundred bars in
 * again. The chart oscillates for as long as bars keep closing.
 *
 * Never narrows: `from` is whichever edge reaches further back. */
export function reloadWindow(
	loaded: readonly { ts_open: number }[],
	initial: { from: number; to: number }
): { from: number; to: number } {
	const first = loaded[0];
	if (!first) return initial;
	return { from: Math.min(first.ts_open, initial.from), to: initial.to };
}
