// Keeps a pane's main chart and its own sub-pane indicator charts moving
// together on the time axis (a sub-pane whose range drifts from the price
// chart above it is not a sub-pane, it is a second chart that happens to
// sit below). Mirrors the design reference's own `linkSub`/`syncSubs`
// (lines 1898-1915): each pair gets a shared guard flag so a mirrored write
// never re-triggers the handler that produced it — without that, the two
// charts' `subscribeVisibleLogicalRangeChange` handlers would call each
// other forever the instant either range changed.
import type { LogicalRange, LogicalRangeChangeEventHandler } from 'lightweight-charts';

/** The slice of `lightweight-charts`' `ITimeScaleApi` this module actually
 * needs — kept narrow (rather than importing the concrete type) so a test
 * can exercise the guard logic against a plain fake, with no real chart or
 * DOM involved. */
export interface TimeScaleLike {
	getVisibleLogicalRange(): LogicalRange | null;
	setVisibleLogicalRange(range: LogicalRange): void;
	subscribeVisibleLogicalRangeChange(handler: LogicalRangeChangeEventHandler): void;
	unsubscribeVisibleLogicalRangeChange(handler: LogicalRangeChangeEventHandler): void;
}

/** The slice of `IChartApi` this module needs. */
export interface ChartLike {
	timeScale(): TimeScaleLike;
}

/**
 * Mirrors `a`'s and `b`'s visible logical range onto each other, in either
 * direction, so whichever one the user actually drags or zooms leads that
 * one gesture and the other always catches up. Returns a disposer that
 * unsubscribes both sides — call it once either chart is torn down.
 */
export function linkTimeScales(a: ChartLike, b: ChartLike): () => void {
	const tsA = a.timeScale();
	const tsB = b.timeScale();
	// Shared by both directions, exactly like the reference's one `guard`
	// object per linked pair: while a mirrored write from one side is in
	// flight, the change event that write itself fires on the other side is
	// ignored, so a range change can propagate leader -> follower -> (a
	// sibling follower, via the leader) without ever bouncing back into
	// whichever chart the user is actually touching.
	let guarding = false;

	const follow = (target: TimeScaleLike): LogicalRangeChangeEventHandler =>
		(range) => {
			if (guarding || !range) return;
			guarding = true;
			try {
				target.setVisibleLogicalRange(range);
			} catch {
				// the target chart may already be mid-teardown.
			} finally {
				guarding = false;
			}
		};

	const onA = follow(tsB);
	const onB = follow(tsA);
	tsA.subscribeVisibleLogicalRangeChange(onA);
	tsB.subscribeVisibleLogicalRangeChange(onB);

	const initial = tsA.getVisibleLogicalRange();
	if (initial) {
		try {
			tsB.setVisibleLogicalRange(initial);
		} catch {
			// `b` may not be laid out yet.
		}
	}

	return () => {
		tsA.unsubscribeVisibleLogicalRangeChange(onA);
		tsB.unsubscribeVisibleLogicalRangeChange(onB);
	};
}
