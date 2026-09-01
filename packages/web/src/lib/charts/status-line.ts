// What a pane's status line shows for one bar, as data rather than as a
// pre-formatted string.
//
// The chart knows the numbers and the pane header knows how much room it
// has, so neither of them is the right place to decide *which* of those
// numbers a reader asked to see — that is six stored settings
// (`statusOhlc`, `statusChange`, `statusVolume` and the three the header
// itself applies), and deciding it here is what makes it testable without a
// chart or a DOM.

import type { BarDto } from '$lib/api/types';
import type { ChartSettings } from '$lib/mock/chart-settings';

/** One bar's numbers, already decimal — the caller has divided out the
 * instrument's `price_scale`/`qty_scale` before this sees them. */
export interface StatusBar {
	open: number;
	high: number;
	low: number;
	close: number;
	/** The close of the bar before this one, or `null` for the first bar of
	 * a window — there is no change to report against nothing. */
	previousClose: number | null;
	/** Base-asset volume, or a tick count, or `null` when the venue reports
	 * neither. The two are not interchangeable: a tick count is a number of
	 * price changes, not a quantity, and labelling it "VOL" would be a
	 * different fact than the one the venue sent. */
	volume: { kind: 'real'; value: number } | { kind: 'tick'; value: number } | null;
}

/** One labelled cell of the status line. `tone` is only ever `up`/`down` on
 * a value whose sign means something — never on a price. */
export interface StatusSegment {
	label: string;
	value: string;
	tone: 'up' | 'down' | 'neutral';
}

/** How much room the header has. A pane can be narrow enough that four
 * price cells do not fit; the close alone still does. */
export interface StatusLineSpace {
	/** Decimal places for a price. */
	precision: number;
	/** Whether to drop everything but the close from the OHLC group. */
	compact: boolean;
}

/** Volume, short enough to sit in a chart header: `1.2K`, `3.45M`, `812`.
 * A tick count is left whole — it is small, and rounding a count of trades
 * to `1.2K` loses the only thing it says. */
export function formatVolume(volume: NonNullable<StatusBar['volume']>): string {
	if (volume.kind === 'tick') return String(Math.round(volume.value));
	const value = volume.value;
	const abs = Math.abs(value);
	if (abs >= 1_000_000_000) return `${(value / 1_000_000_000).toFixed(2)}B`;
	if (abs >= 1_000_000) return `${(value / 1_000_000).toFixed(2)}M`;
	if (abs >= 1_000) return `${(value / 1_000).toFixed(1)}K`;
	return value.toFixed(abs >= 1 ? 2 : 4);
}

/** The change from the previous bar's close, absolute and as a percentage,
 * or `null` when there is no previous close to measure against.
 *
 * Against the previous *close*, not this bar's own open — that is what a
 * change column means everywhere else, and the two differ by the gap. */
export function barChange(
	bar: StatusBar
): { absolute: number; percent: number | null } | null {
	if (bar.previousClose === null) return null;
	const absolute = bar.close - bar.previousClose;
	// A previous close of zero is not a real price; a percentage against it
	// would be `Infinity`, which is worse than saying nothing.
	const percent = bar.previousClose === 0 ? null : (absolute / bar.previousClose) * 100;
	return { absolute, percent };
}

function toneOf(value: number): 'up' | 'down' | 'neutral' {
	if (value > 0) return 'up';
	if (value < 0) return 'down';
	return 'neutral';
}

/** The status line's cells for `bar`, in reading order, honouring the
 * pane's stored settings. An empty array is a valid answer — a reader who
 * turned every group off gets a header with no readout at all, which is the
 * point of the settings. */
export function statusLineSegments(
	bar: StatusBar,
	settings: Pick<ChartSettings, 'statusOhlc' | 'statusChange' | 'statusVolume'>,
	space: StatusLineSpace
): StatusSegment[] {
	const price = (value: number) => value.toFixed(space.precision);
	const segments: StatusSegment[] = [];

	if (settings.statusOhlc) {
		if (space.compact) {
			segments.push({ label: 'C', value: price(bar.close), tone: 'neutral' });
		} else {
			segments.push(
				{ label: 'O', value: price(bar.open), tone: 'neutral' },
				{ label: 'H', value: price(bar.high), tone: 'neutral' },
				{ label: 'L', value: price(bar.low), tone: 'neutral' },
				{ label: 'C', value: price(bar.close), tone: 'neutral' }
			);
		}
	}

	if (settings.statusChange) {
		const change = barChange(bar);
		if (change) {
			const sign = change.absolute > 0 ? '+' : '';
			const percent = change.percent === null ? '' : ` (${sign}${change.percent.toFixed(2)}%)`;
			segments.push({
				label: '',
				value: `${sign}${price(change.absolute)}${percent}`,
				tone: toneOf(change.absolute)
			});
		}
	}

	if (settings.statusVolume && bar.volume) {
		segments.push({
			label: bar.volume.kind === 'tick' ? 'TICKS' : 'VOL',
			value: formatVolume(bar.volume),
			tone: 'neutral'
		});
	}

	return segments;
}

/** One bar's wire volume as the status line needs it: a decimal base-asset
 * quantity (the venue's integer divided by the series' `qty_scale`), a raw
 * tick count, or `null` when the venue reported none.
 *
 * Only the `real` kind is scaled. A tick count is a count of price changes,
 * not a quantity, and has no scale to divide by — dividing it would turn
 * 1,234 trades into 0.00001234 of nothing. */
export function statusVolumeFromDto(
	volume: BarDto['volume'],
	qtyScale: number
): StatusBar['volume'] {
	if (volume.kind === 'real') return { kind: 'real', value: volume.value / 10 ** qtyScale };
	if (volume.kind === 'tick') return { kind: 'tick', value: volume.value };
	return null;
}
