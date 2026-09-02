// Which position and working-order price lines a chart pane should draw —
// pure, so the "which lines exist" decision is unit-testable without
// `lightweight-charts` in the loop at all. The drawing half (creating,
// diffing and removing `IPriceLine`s on a real `ISeriesApi`) lives in
// `chart-pane.svelte`'s own `$effect`, which reads `tradeStore`'s *current*
// value on every run — never a snapshot captured when the pane mounted. See
// `AGENTS.md`'s "Derive from what you hold, not from what you opened with":
// this is the same chart, and the same bug class, the store's five-second
// refresh tick would reproduce if a line set were ever computed once and
// left alone.

import type { OrderDto, PositionDto, ScaledDto } from '$lib/api/types';
import { isWorking } from '$lib/trade/view';
import { formatScaled, formatScaledForDisplay } from '$lib/trade/scaled';

/** One price line a chart pane draws for the account's own position or
 * working orders on this pane's instrument. */
export interface TradeLine {
	/** Stable identity, so a redraw can diff rather than clear and rebuild. */
	key: string;
	/** The price, already converted to the number the chart series uses. */
	price: number;
	/** `'LONG 0.25'` for a position, `'BUY LIMIT 0.25'` for a working order. */
	title: string;
	kind: 'position' | 'order';
	tone: 'gain' | 'loss' | 'neutral';
	/** Orders are dashed, positions solid — the same visual language the
	 * pane's other guide lines (high/low, bid/ask) already use for
	 * "provisional" versus "resting" levels. */
	dashed: boolean;
}

/** Converts a scaled price to the plain number `lightweight-charts` draws
 * at — a chart series takes `number`, so this conversion is unavoidable
 * somewhere, and this is the one place in the whole pane it happens. The
 * value that comes out is drawn, never traded: nothing downstream of this
 * function may feed an order.
 *
 * `formatScaled` (never `formatScaledForDisplay`, which rounds to 2
 * fractional digits by default) is what keeps this exact — an eight-decimal
 * price silently rounded to cents would draw a line the position is not
 * actually at. `.toFixed(priceScale)` then matches the exact precision the
 * pane's own candles are computed at (`bar.close / 10 ** priceScale` in
 * `chart-pane.svelte`), so a line and the candle beside it never disagree
 * on a trailing digit neither of them intends to show. */
function scaledToChartNumber(scaled: ScaledDto, priceScale: number): number {
	const exact = Number(formatScaled(scaled));
	return priceScale > 0 ? Number(exact.toFixed(priceScale)) : Math.round(exact);
}

/** Every line the pane showing `instrument` should carry, from the
 * positions and working orders **currently** in the store — this function
 * takes them as plain arrays precisely so its caller is the one that must
 * read `tradeStore` fresh on every call, never hold a copy across calls. */
export function tradeLinesFor(
	instrument: string,
	positions: PositionDto[],
	orders: OrderDto[],
	priceScale: number
): TradeLine[] {
	const lines: TradeLine[] = [];

	for (const position of positions) {
		if (position.instrument !== instrument) continue;
		lines.push({
			key: `position:${position.account_id}:${position.instrument}`,
			price: scaledToChartNumber(position.average_entry, priceScale),
			title: `${position.side.toUpperCase()} ${formatScaledForDisplay(position.quantity, 8)}`,
			kind: 'position',
			tone: position.side === 'short' ? 'loss' : 'gain',
			dashed: false
		});
	}

	for (const order of orders) {
		if (order.instrument !== instrument) continue;
		if (!isWorking(order)) continue;
		// A limit order's own price when it has one; otherwise the trigger a
		// stop is waiting on. A market order in flight has neither and never
		// reaches "working" long enough to matter, but a well-formed working
		// order missing both is not representable as a level and is skipped
		// rather than drawn at a made-up price.
		const priceSource = order.limit_price ?? order.trigger_price;
		if (!priceSource) continue;
		lines.push({
			key: `order:${order.account_id}:${order.id}`,
			price: scaledToChartNumber(priceSource, priceScale),
			title: `${order.side.toUpperCase()} ${order.kind.replace('_', ' ').toUpperCase()} ${formatScaledForDisplay(order.quantity, 8)}`,
			kind: 'order',
			tone: 'neutral',
			dashed: true
		});
	}

	return lines;
}
