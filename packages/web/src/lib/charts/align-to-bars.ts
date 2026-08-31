// Puts a sub-pane indicator on exactly the bars the main chart is showing.
//
// Main and sub-pane are separate chart instances that share a *logical*
// range, and a logical index counts from each chart's own first data point.
// An indicator with a warm-up — RSI(14) publishes nothing for the first 14
// bars — therefore starts its own index 0 at the main chart's bar 14, and
// copying the range across shifts the whole strip left by the warm-up
// length. It looks like a rendering bug and is really an off-by-warm-up in
// what each chart calls "bar 0".
//
// So the sub-pane is given one entry per main bar, in the main chart's own
// order: a value where the indicator has one, a whitespace point where it
// does not. Alignment then holds by construction rather than by the two
// series happening to begin together.
import type { UTCTimestamp } from 'lightweight-charts';

/** A point carrying a value, or a gap that only reserves its slot. */
export type AlignedPoint = { time: UTCTimestamp; value: number } | { time: UTCTimestamp };

/**
 * Maps `points` onto `barTimes`, in that order.
 *
 * A point whose time is not one of `barTimes` is dropped: the main chart is
 * not showing that bar, so nothing in the sub-pane may sit at it.
 */
export function alignToBarTimes(
	barTimes: readonly UTCTimestamp[],
	points: readonly { time: number; value: number }[]
): AlignedPoint[] {
	const byTime = new Map<number, number>();
	for (const point of points) byTime.set(point.time, point.value);
	return barTimes.map((time) => {
		const value = byTime.get(time as unknown as number);
		return value === undefined ? { time } : { time, value };
	});
}
