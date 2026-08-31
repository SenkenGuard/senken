// Static UI vocabulary for the charts page.
//
// `$lib/mock/charts.ts` used to hold two very different kinds of thing under
// one "mock" name: fixture *data* pretending to be a snapshot of something
// real (candles, indicator series, a fake workspace list) — which
// B2/B7 replace with real server calls — and plain UI *vocabulary* that was
// never data at all: which draw tools exist, which pane-grid presets the
// layout menu offers, which tabs the right panel has. That second kind
// belongs here instead of disappearing along with the mock module (see
// `routes/charts/+page.svelte`'s header comment for where the real data
// moved to).
//
// `LayoutId`/`LAYOUTS` deliberately keep the exact tokens
// `senken_workspace::LayoutPreset::{Display,FromStr}`
// (`crates/workspace/src/preset.rs`) round-trips through on the wire — this
// is this project's one shared vocabulary for a pane-grid arrangement, not
// a client-side reinvention of it.

import type { Component } from 'svelte';
import MousePointer2Icon from '@lucide/svelte/icons/mouse-pointer-2';
import CrosshairIcon from '@lucide/svelte/icons/crosshair';
import SlashIcon from '@lucide/svelte/icons/slash';
import MinusIcon from '@lucide/svelte/icons/minus';
import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
import Rows3Icon from '@lucide/svelte/icons/rows-3';
import RectangleHorizontalIcon from '@lucide/svelte/icons/rectangle-horizontal';
import TargetIcon from '@lucide/svelte/icons/target';
import TypeIcon from '@lucide/svelte/icons/type';
import RulerIcon from '@lucide/svelte/icons/ruler';
import CoinsIcon from '@lucide/svelte/icons/coins';
import SigmaIcon from '@lucide/svelte/icons/sigma';
import ZapIcon from '@lucide/svelte/icons/zap';
import ListIcon from '@lucide/svelte/icons/list';
import BellIcon from '@lucide/svelte/icons/bell';
import LayersIcon from '@lucide/svelte/icons/layers';
import PenLineIcon from '@lucide/svelte/icons/pen-line';
import SlidersHorizontalIcon from '@lucide/svelte/icons/sliders-horizontal';

// --- Draw toolbar (unchanged from the former mock/charts.ts) ---------------

export type ToolKey = 'cursor' | 'cross' | 'trend' | 'hline' | 'ray' | 'fib' | 'rect' | 'pos' | 'text' | 'measure';

export interface DrawTool {
	key: ToolKey;
	icon: Component;
	label: string;
}

export const DRAW_TOOLS: DrawTool[] = [
	{ key: 'cursor', icon: MousePointer2Icon, label: 'CURSOR' },
	{ key: 'cross', icon: CrosshairIcon, label: 'CROSSHAIR' },
	{ key: 'trend', icon: SlashIcon, label: 'TREND LINE' },
	{ key: 'hline', icon: MinusIcon, label: 'HORIZONTAL LINE' },
	{ key: 'ray', icon: ArrowRightIcon, label: 'RAY' },
	{ key: 'fib', icon: Rows3Icon, label: 'FIB RETRACEMENT' },
	{ key: 'rect', icon: RectangleHorizontalIcon, label: 'RECTANGLE ZONE' },
	{ key: 'pos', icon: TargetIcon, label: 'LONG / SHORT POSITION' },
	{ key: 'text', icon: TypeIcon, label: 'TEXT NOTE' },
	{ key: 'measure', icon: RulerIcon, label: 'MEASURE' }
];

/** The drawing tools that create a real, persisted drawing object
 * (`chart-pane.svelte`'s click handler, `$lib/charts/drawing-primitive.ts`'s
 * renderer/hit-test) — horizontal line, trend line, rectangle zone. The
 * owner's own instruction: fewer tools that all behave correctly beats more
 * that half-work. The remaining draw tools (ray, fib, position, text,
 * measure) stay selectable but do not draw anything yet — a fourth object
 * kind needs the same create/hit-test/select/edit/delete/persist treatment
 * as these three before it belongs here. */
export const OBJECT_DRAW_TOOLS: ToolKey[] = ['hline', 'trend', 'rect'];

// --- Layout presets ---------------------------------------------------------

export type LayoutId = '1' | '2h' | '2v' | '3h' | '3v' | '4';

export interface LayoutDef {
	id: LayoutId;
	cols: string;
	rows: string;
	n: number;
}

export const LAYOUTS: Record<LayoutId, LayoutDef> = {
	'1': { id: '1', cols: '1fr', rows: '1fr', n: 1 },
	'2h': { id: '2h', cols: '1fr 1fr', rows: '1fr', n: 2 },
	'2v': { id: '2v', cols: '1fr', rows: '1fr 1fr', n: 2 },
	'3h': { id: '3h', cols: '1fr 1fr 1fr', rows: '1fr', n: 3 },
	'3v': { id: '3v', cols: '1fr', rows: '1fr 1fr 1fr', n: 3 },
	'4': { id: '4', cols: '1fr 1fr', rows: '1fr 1fr', n: 4 }
};

export const LAYOUT_GROUPS: { count: number; ids: LayoutId[] }[] = [
	{ count: 1, ids: ['1'] },
	{ count: 2, ids: ['2h', '2v'] },
	{ count: 3, ids: ['3h', '3v'] },
	{ count: 4, ids: ['4'] }
];

// --- Timeframes --------------------------------------------------------------

/** The wire tokens `senken_series::BarSpec::{Display,FromStr}`
 * (`crates/series/src/spec.rs`) round-trips through — a lower-case integer
 * step plus a single-letter unit suffix (`"mo"` for month, unused here).
 * Sent to `apiClient.rangeBars`/`ensureBars`/`planBars` verbatim; `tfLabel`
 * below is the only place this gets upper-cased, for display. */
export const TF_LIST = ['1m', '5m', '15m', '1h', '4h', '1d', '1w'] as const;
export type Timeframe = (typeof TF_LIST)[number];

export function tfLabel(tf: string): string {
	return tf.toUpperCase();
}

// --- Right panel tabs --------------------------------------------------------

export type ChartPanelKey = 'engine' | 'watch' | 'alerts' | 'objects' | 'notes';

export const PANEL_TABS: { key: ChartPanelKey; icon: Component; label: string }[] = [
	{ key: 'engine', icon: ZapIcon, label: 'TRADE ENGINE' },
	{ key: 'watch', icon: ListIcon, label: 'WATCHLIST' },
	{ key: 'alerts', icon: BellIcon, label: 'ALERTS' },
	{ key: 'objects', icon: LayersIcon, label: 'OBJECT TREE' },
	{ key: 'notes', icon: PenLineIcon, label: 'NOTES' }
];

export const CHART_MENUS: { key: 'ind' | 'set'; icon: Component; label: string }[] = [
	{ key: 'ind', icon: SigmaIcon, label: 'INDICATORS & LAYERS' },
	{ key: 'set', icon: SlidersHorizontalIcon, label: 'CHART SETTINGS' }
];

// --- Layers ------------------------------------------------------------------

/** The three shapes `senken_workspace::LayerKind`
 * (`crates/workspace/src/store.rs`) actually stores — this project's one
 * vocabulary for "what kind of layer is this", matching
 * `$lib/api/types.ts`'s generated `LayerKindDto` tag exactly. Unlike the
 * former mock model, a pane's *main* instrument is not a layer at all: it
 * is the pane's own `instrument`/`timeframe` fields (see
 * `$lib/charts/pane-runtime.ts`). */
export type LayerKindTag = 'overlay_instrument' | 'indicator_overlay' | 'indicator_sub_pane';

export const LAYER_KIND_ICON: Record<LayerKindTag, Component> = {
	overlay_instrument: CoinsIcon,
	indicator_overlay: SigmaIcon,
	indicator_sub_pane: SigmaIcon
};

/** One entry in the "add layer" picker's INDICATOR tab. `name` is exactly
 * the string `GET /api/indicators` catalogues and
 * `senken_alerts::ConcreteIndicator::build` accepts (one source of truth for the maths, never a browser-side second one).
 * `defaultParams` seeds `POST /api/layouts/{id}`'s `params` JSON text with
 * the same conventional defaults `crates/indicators`' own hand-computed
 * tests use; `subPane` decides `IndicatorOverlay` vs `IndicatorSubPane` at
 * the moment a layer is created — mirroring "MACD and RSI are sub-pane;
 * Bollinger and the moving averages are overlays",
 * extended to Stochastic/Atr the same way `crates/indicators`' own doc
 * comments group momentum oscillators together. */
export interface IndicatorCatalogItem {
	name: string;
	label: string;
	paramsLabel: string;
	defaultParams: Record<string, number>;
	subPane: boolean;
}

export const INDICATOR_CATALOG: IndicatorCatalogItem[] = [
	{ name: 'Ema', label: 'EMA', paramsLabel: '50 · close', defaultParams: { period: 50 }, subPane: false },
	{ name: 'Sma', label: 'SMA', paramsLabel: '20 · close', defaultParams: { period: 20 }, subPane: false },
	{ name: 'Wma', label: 'WMA', paramsLabel: '20 · close', defaultParams: { period: 20 }, subPane: false },
	{
		name: 'BollingerBands',
		label: 'BOLLINGER BANDS',
		paramsLabel: '20 · 2.0 · overlay',
		defaultParams: { period: 20, k: 2 },
		subPane: false
	},
	{ name: 'Vwap', label: 'VWAP', paramsLabel: 'session', defaultParams: {}, subPane: false },
	{ name: 'Volume', label: 'VOLUME', paramsLabel: 'overlay', defaultParams: {}, subPane: false },
	{ name: 'Rsi', label: 'RSI', paramsLabel: '14 · sub-pane', defaultParams: { period: 14 }, subPane: true },
	{
		name: 'Macd',
		label: 'MACD',
		paramsLabel: '12 26 9 · sub-pane',
		defaultParams: { fast_period: 12, slow_period: 26, signal_period: 9 },
		subPane: true
	},
	{
		name: 'Stochastic',
		label: 'STOCHASTIC',
		paramsLabel: '14 3 · sub-pane',
		defaultParams: { k_period: 14, d_period: 3 },
		subPane: true
	},
	{ name: 'Atr', label: 'ATR', paramsLabel: '14 · sub-pane', defaultParams: { period: 14 }, subPane: true }
];

// --- Instruments ---------------------------------------------------------
//
// Replaces the former fixed four-symbol placeholder with real
// search against `GET /api/instruments` (`apiClient.searchInstruments`,
// `InstrumentSummaryDto` in `$lib/api/types.ts`) — see
// `routes/charts/+page.svelte`'s instrument-search wiring and
// `workspace-store.svelte.ts`'s `nextInstrument`.

/** `"binance-spot:BTCUSDT"` -> `{ venue: "BINANCE-SPOT", ticker: "BTCUSDT" }`
 * — `senken_marketdata::InstrumentId`'s own wire form is `source:symbol`
 * (`crates/api/src/bars_handlers.rs`'s doc comments), so this is a plain
 * display parse of that string, not a catalog lookup. */
export function parseInstrumentId(id: string): { venue: string; ticker: string } {
	const [source, symbol] = id.split(':');
	return { venue: (source ?? id).toUpperCase(), ticker: symbol ?? id };
}

/** How to turn one `POST /api/indicators/compute` field value back into a
 * real number, given the instrument's `price_scale`/`qty_scale` (the same
 * pair `GET /api/bars/range` reports on `BarRangeResponse`).
 *
 * `crates/indicators/src/convert.rs`'s `scaled_to_f64` is a **plain
 * widening cast** — it does not divide by any scale — so an indicator that
 * only ever adds/subtracts/averages `Bar`'s scaled price fields (an EMA, an
 * SMA, Bollinger Bands, MACD's line/signal/histogram, VWAP, ATR's true
 * range) reports its value still multiplied by `10^price_scale`, exactly
 * like `BarDto` itself. Volume is the same story at `qty_scale`. RSI and
 * Stochastic are the two exceptions: both are *ratios* of same-scale price
 * deltas (`avgGain/avgLoss`, `(close-lowest)/(highest-lowest)`), so the
 * scale factor cancels out of the division and the reported 0–100 value is
 * already real — dividing it again would be wrong, not merely redundant. */
export function indicatorFieldScale(indicatorName: string, field: string): 'price' | 'qty' | 'none' {
	if (field === 'stochastic_k' || field === 'stochastic_d') return 'none';
	if (indicatorName === 'Rsi') return 'none';
	if (indicatorName === 'Volume') return 'qty';
	return 'price';
}

/** The real-time duration of one of `TF_LIST`'s tokens, in seconds — used
 * to size the default `[from, to)` window a pane requests bars for. */
export const TF_DURATION_SECONDS: Record<Timeframe, number> = {
	'1m': 60,
	'5m': 300,
	'15m': 900,
	'1h': 3600,
	'4h': 14400,
	'1d': 86400,
	'1w': 604800
};

/** The `[from, to)` nanosecond window a pane requests by default: the most
 * recent `count` bars of `spec`, ending "now" aligned down to a spec-sized
 * boundary. Plain `number` (not `bigint`) throughout this file and its
 * callers: `Number` only loses precision below roughly 2048ns at today's
 * epoch magnitude (`Number.MAX_SAFE_INTEGER` is ~9e15, "now" in nanoseconds
 * is ~1.7e18), and every consumer here only ever floors to whole seconds —
 * five orders of magnitude coarser than where that loss would matter. */
export function defaultBarWindow(spec: string, count = 300): { from: number; to: number } {
	const stepSeconds = TF_DURATION_SECONDS[spec as Timeframe] ?? 3600;
	const nowSeconds = Math.floor(Date.now() / 1000 / stepSeconds) * stepSeconds;
	const toNanos = nowSeconds * 1_000_000_000;
	const fromNanos = toNanos - count * stepSeconds * 1_000_000_000;
	return { from: fromNanos, to: toNanos };
}

/** Whether `spec` is finer than a day — governs whether
 * lightweight-charts' time axis (and crosshair time label) should show a
 * clock time or just a date. `timeVisible` defaults to `false` in
 * `TimeScaleOptions` (dates only, which is why every intraday tick used to
 * repeat the same day-of-month with no time at all), and turning it on
 * without also forcing `secondsVisible: false` (which itself defaults to
 * `true`) would print `14:15:00` on every spec coarser than a second — so
 * both are driven from this one check rather than left at either default. */
export function isIntradaySpec(spec: string): boolean {
	return spec !== '1d' && spec !== '1w';
}

/** Renders a bar price (a scaled integer, `senken_series::Bar`'s own wire
 * form) back to a decimal string given the instrument's `price_scale` —
 * `crates/api/src/dto.rs`'s `BarDto` doc: "a client must already know an
 * instrument's price_scale ... to render these as real prices." */
export function scaledToDisplay(value: number, scale: number): string {
	return fmtDecimal(value / 10 ** scale);
}

/** Formats an already-decimal price/quantity the same way `scaledToDisplay`
 * formats a scaled integer, for values that never went through the
 * `price_scale` wire encoding in the first place (e.g. the trade-engine
 * demo's own derived prices — see `terminal/trade-demo.ts`). */
export function fmtDecimal(decimal: number): string {
	if (decimal >= 1000) return decimal.toLocaleString('en-US', { maximumFractionDigits: 2, minimumFractionDigits: 2 });
	if (decimal >= 10) return decimal.toFixed(2);
	return decimal.toFixed(4);
}

/** The time-axis label format, shared by a pane's main chart and every
 * sub-pane under it.
 *
 * One function rather than one per chart: a sub-pane shows the same bars as
 * its main chart, so an axis that labelled them differently would read as a
 * different series. `timeVisible` alone is not enough — the library's own
 * default formats an intraday tick as a bare day number.
 */
export function timeAxisFormatter(
	spec: string,
	dayOfWeek: boolean,
	timeFormat: '24H' | '12H'
): (time: number) => string {
	const intraday = isIntradaySpec(spec);
	return (time: number) => {
		const at = new Date(time * 1000);
		const weekday = at.toLocaleDateString(undefined, { weekday: 'short' });
		if (!intraday) {
			const date = at.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
			return dayOfWeek ? `${weekday} ${date}` : date;
		}
		const clock = at.toLocaleTimeString(undefined, {
			hour: '2-digit',
			minute: '2-digit',
			hour12: timeFormat === '12H'
		});
		return dayOfWeek ? `${weekday} ${clock}` : clock;
	};
}
