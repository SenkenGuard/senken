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

/** What a link hands back: a way to re-assert the leader's range, and a
 * way to take the link down. */
export interface LinkedTimeScales {
	/** Runs `apply` — a data update on `b` — with `b`'s own range
	 * announcements suppressed, then re-asserts `a`'s range on it.
	 *
	 * A chart handed a new series settles on a range of its own choosing
	 * and announces it, and that announcement is indistinguishable from a
	 * gesture. It is also raised *inside* the data call, so re-asserting
	 * afterwards is too late — the leader has already been moved, and the
	 * re-assert then spreads that moved range instead of correcting it.
	 * The suppression has to span the update itself, which is why this
	 * takes the update rather than bracketing it. */
	applyToFollower(apply: () => void): void;
	/** Unsubscribes both sides — call it once either chart is torn down. */
	dispose(): void;
}

/**
 * Mirrors `a`'s and `b`'s visible logical range onto each other, in either
 * direction, so whichever one the user actually drags or zooms leads that
 * one gesture and the other always catches up.
 */
export function linkTimeScales(a: ChartLike, b: ChartLike): LinkedTimeScales {
	const tsA = a.timeScale();
	const tsB = b.timeScale();
	// Shared by both directions, exactly like the reference's one `guard`
	// object per linked pair: while a mirrored write from one side is in
	// flight, the change event that write itself fires on the other side is
	// ignored, so a range change can propagate leader -> follower -> (a
	// sibling follower, via the leader) without ever bouncing back into
	// whichever chart the user is actually touching.
	// A depth counter rather than a flag: a suppressed data update on the
	// follower contains its own guarded writes, and a flag would be cleared
	// by the inner one while the outer suppression is still meant to hold.
	let depth = 0;
	const guarded = (write: () => void) => {
		depth += 1;
		try {
			write();
		} catch {
			// the target chart may already be mid-teardown.
		} finally {
			depth -= 1;
		}
	};

	const follow = (target: TimeScaleLike): LogicalRangeChangeEventHandler =>
		(range) => {
			if (depth > 0 || !range) return;
			guarded(() => target.setVisibleLogicalRange(range));
		};

	const onA = follow(tsB);
	const onB = follow(tsA);
	tsA.subscribeVisibleLogicalRangeChange(onA);
	tsB.subscribeVisibleLogicalRangeChange(onB);

	// Guarded, exactly like a mirrored write. Handing `b` the leader's range
	// makes `b` announce a range change, and an unguarded hand-off lets that
	// announcement travel back and overwrite the leader — so linking a chart
	// moved the very chart it was supposed to follow. A sub-pane is linked
	// while it is still empty, and what an empty chart makes of a logical
	// range is not the leader's range at all.
	const push = () => {
		const range = tsA.getVisibleLogicalRange();
		if (!range) return;
		guarded(() => tsB.setVisibleLogicalRange(range));
	};
	push();

	return {
		applyToFollower: (apply: () => void) => {
			depth += 1;
			try {
				apply();
			} finally {
				// Still suppressed while the leader's range is restored, so
				// the restore cannot echo either; released only after.
				push();
				depth -= 1;
			}
		},
		dispose: () => {
			tsA.unsubscribeVisibleLogicalRangeChange(onA);
			tsB.unsubscribeVisibleLogicalRangeChange(onB);
		}
	};
}
