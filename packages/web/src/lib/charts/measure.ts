// Pure delta math for the measure tool — the price/percent/bar-count
// readout shown between two clicked points. Deliberately outside
// `./drawable.ts`/`./drawing-hit.ts`: a measurement is never persisted and
// never selected or dragged, so it carries none of `ObjectDrawable`'s
// owner-id/style machinery, and its geometry math (a delta between two
// points, not a shape to paint or hit-test) is its own small vocabulary —
// widening `ObjectDrawable` with a fifth, unselectable, unpersisted variant
// would make every exhaustive switch over it (`drawing-hit.ts`,
// `drawing-primitive.ts`) carry a case that can never fire.
import type { DrawablePoint } from './drawable';

/** The raw numbers a measurement reduces two points to. */
export interface MeasureStats {
	/** `b.value - a.value`, in the chart's own (already-decimal) price units. */
	priceDelta: number;
	/** `priceDelta` as a percentage of `a.value` — `0` if `a.value` is `0`,
	 * since a percentage of a zero base is not a meaningful number to show. */
	percent: number;
	/** `b.time - a.time`, in seconds — negative if `b` is earlier than `a`. */
	seconds: number;
	/** `seconds` divided by the pane's own bar step, rounded to the nearest
	 * whole bar — the two anchors need not land exactly on plotted bars. */
	bars: number;
}

/** Reduces two (time, price) points to the numbers `formatMeasureLabel`
 * renders — pure, so a dragged measurement can be recomputed on every mouse
 * move without touching a chart or a canvas. */
export function measureStats(a: DrawablePoint, b: DrawablePoint, stepSeconds: number): MeasureStats {
	const priceDelta = b.value - a.value;
	const percent = a.value === 0 ? 0 : (priceDelta / a.value) * 100;
	const seconds = b.time - a.time;
	const bars = stepSeconds > 0 ? Math.round(seconds / stepSeconds) : 0;
	return { priceDelta, percent, bars, seconds };
}

/** Renders `stats` as the single line of text painted next to the pointer —
 * matches the sign convention and decimal precision `chart-pane.svelte`'s
 * own crosshair OHLC readout already uses (`bar.close.toFixed(4)`), so a
 * measurement's numbers look like they belong on the same chart. */
export function formatMeasureLabel(stats: MeasureStats): string {
	const sign = stats.priceDelta >= 0 ? '+' : '';
	const bars = Math.abs(stats.bars) === 1 ? 'bar' : 'bars';
	return `${sign}${stats.priceDelta.toFixed(4)}  ${sign}${stats.percent.toFixed(2)}%  ${stats.bars} ${bars}`;
}
