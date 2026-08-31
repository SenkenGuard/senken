// Mock data for the dashboard page, plus
// the derived dashboard props from the state/mock script block: CATALOG at
// line 1353, the initial `workspaces` at line 1560, the mulberry-seeded
// equity/heatmap/flow generators at lines 2185-2280, and the
// watchlist/positions/signals/feed fixtures at lines 3103-3145.
//
// No `fetch`, no `/api` calls: this page is UI-only, so every value here is a
// fixture, not a snapshot of anything real. The equity spark line, benchmark
// line, volatility heatmap and flow bars reproduce the reference's own
// `mulberry32`-seeded pseudo-random walks verbatim (same seeds, same
// formulas) so the shapes rendered here match the reference exactly rather
// than being invented.
//
// Deviation from the reference: `lastPrice()` in the reference comes from
// `genCandles`, a seeded OHLC simulator that belongs to the charts page
// (`chart-pane` owns the candle simulation). Duplicating
// that simulator here would cross the P2/P3 file boundary, so positions and
// the watchlist use each symbol's fixture `base` price directly instead —
// the same numbers `genCandles` would settle near, without importing its
// code.

export type WidgetType =
	| 'equity'
	| 'watchlist'
	| 'positions'
	| 'risk'
	| 'heatmap'
	| 'signals'
	| 'flow'
	| 'feed';

export type Tone = 'gain' | 'loss' | 'neutral';

/** One widget placed in a workspace (reference: `workspaces[].widgets[]`). */
export interface WidgetInstance {
	id: string;
	type: WidgetType;
	span: number;
}

/** One dashboard workspace / layout (reference: `state.workspaces`). */
export interface Workspace {
	name: string;
	widgets: WidgetInstance[];
}

/** Widget catalog entry (reference: `CATALOG`, line 1353) — the metadata
 * used both by the "ADD WIDGET" picker and by each placed widget's header. */
export interface CatalogEntry {
	type: WidgetType;
	title: string;
	span: number;
	hint: string;
	/** reference: `w.type === 'equity' ? '272px' : (positions/watchlist ? '272px' : '236px')` */
	minHeightClass: string;
}

export const CATALOG: CatalogEntry[] = [
	{ type: 'equity', title: 'Equity Curve', span: 6, hint: 'balance + benchmark', minHeightClass: 'min-h-[272px]' },
	{ type: 'watchlist', title: 'Watchlist', span: 3, hint: 'tracked pairs', minHeightClass: 'min-h-[272px]' },
	{ type: 'positions', title: 'Open Positions', span: 6, hint: 'live PnL table', minHeightClass: 'min-h-[272px]' },
	{ type: 'risk', title: 'Risk Meters', span: 3, hint: 'exposure gauges', minHeightClass: 'min-h-[236px]' },
	{ type: 'heatmap', title: 'Volatility Heatmap', span: 4, hint: 'daily grid', minHeightClass: 'min-h-[236px]' },
	{ type: 'signals', title: 'Signal Desk', span: 4, hint: 'model verdicts', minHeightClass: 'min-h-[236px]' },
	{ type: 'flow', title: 'Buy / Sell Flow', span: 4, hint: 'volume split', minHeightClass: 'min-h-[236px]' },
	{ type: 'feed', title: 'News Tape', span: 4, hint: 'headline stream', minHeightClass: 'min-h-[236px]' }
];

export function catalogEntry(type: WidgetType): CatalogEntry {
	return CATALOG.find((c) => c.type === type) ?? CATALOG[0];
}

/** Resize cycle order for a placed widget's "cycle span" control (reference:
 * `cycleWidget`'s `steps`). */
export const SPAN_STEPS = [3, 4, 6, 12];

/** span -> Tailwind `col-span-*` class. Spelled out as literal strings (not
 * built with template interpolation) so Tailwind's content scanner, which
 * only sees this file's text, picks every one of them up. */
export const SPAN_CLASS: Record<number, string> = {
	3: 'col-span-3',
	4: 'col-span-4',
	6: 'col-span-6',
	8: 'col-span-8',
	12: 'col-span-12'
};

/** Initial workspaces (reference: `state.workspaces`, line 1560). Callers
 * must clone this before mutating — it is a shared module-level fixture. */
export const INITIAL_WORKSPACES: Workspace[] = [
	{
		name: 'Macro Desk',
		widgets: [
			{ id: 'w1', type: 'equity', span: 6 },
			{ id: 'w2', type: 'watchlist', span: 3 },
			{ id: 'w3', type: 'risk', span: 3 },
			{ id: 'w4', type: 'positions', span: 6 },
			{ id: 'w5', type: 'signals', span: 3 },
			{ id: 'w6', type: 'feed', span: 3 }
		]
	},
	{
		name: 'Scalping',
		widgets: [
			{ id: 'w7', type: 'watchlist', span: 4 },
			{ id: 'w8', type: 'flow', span: 4 },
			{ id: 'w9', type: 'heatmap', span: 4 },
			{ id: 'w10', type: 'positions', span: 12 }
		]
	},
	{
		name: 'On-Chain',
		widgets: [
			{ id: 'w11', type: 'heatmap', span: 6 },
			{ id: 'w12', type: 'signals', span: 6 },
			{ id: 'w13', type: 'feed', span: 12 }
		]
	}
];

interface Sym {
	pair: string;
	venue: string;
	ticker: string;
	base: number;
	chg: number;
}

// reference: SYMS, line 1267.
const SYMS: Sym[] = [
	{ pair: 'BTC/USDT', venue: 'BINANCE', ticker: 'BTCUSDT', base: 68420, chg: 2.85 },
	{ pair: 'ETH/USDT', venue: 'BINANCE', ticker: 'ETHUSDT', base: 3512, chg: 1.2 },
	{ pair: 'SOL/USDT', venue: 'BINANCE', ticker: 'SOLUSDT', base: 182.4, chg: 3.51 },
	{ pair: 'XAU/USD', venue: 'OANDA', ticker: 'XAUUSD', base: 2412, chg: -0.42 },
	{ pair: 'EUR/USD', venue: 'OANDA', ticker: 'EURUSD', base: 1.0864, chg: 0.18 },
	{ pair: 'SUI/USDT', venue: 'BYBIT', ticker: 'SUIUSDT', base: 7.552, chg: -1.05 },
	{ pair: 'NAS100', venue: 'OANDA', ticker: 'NAS100USD', base: 19842, chg: 0.74 },
	{ pair: 'ARB/USDT', venue: 'BINANCE', ticker: 'ARBUSDT', base: 1.142, chg: -1.36 }
];

function fmt(v: number): string {
	if (v >= 1000) return v.toLocaleString('en-US', { maximumFractionDigits: 2, minimumFractionDigits: 2 });
	if (v >= 10) return v.toFixed(2);
	return v.toFixed(4);
}

function pct(v: number): string {
	return (v >= 0 ? '▲ ' : '▼ ') + Math.abs(v).toFixed(2) + '%';
}

function toneOf(v: number): Tone {
	return v >= 0 ? 'gain' : 'loss';
}

// reference: `function mulberry(seed)`, line 1364 — a mulberry32 PRNG.
// Ported verbatim so the seeded walks below produce the exact same sequence
// as the reference.
function mulberry(seed: number): () => number {
	let a = seed >>> 0;
	return function () {
		a += 0x6d2b79f5;
		let t = a;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

// --- Equity card (reference lines 154-181, values at 3103-3107) ---

export interface EquityData {
	value: string;
	delta: string;
	deltaTone: Tone;
	/** 0-100 domain walk, paired with `min`/`max` below (reference: `p1`). */
	series: number[];
	/** 0-100 domain benchmark walk (reference: `p2`). */
	benchmark: number[];
	min: number;
	max: number;
}

function buildEquitySeries(): { series: number[]; benchmark: number[] } {
	const eq = mulberry(41);
	let ev = 40;
	let bv = 40;
	const series: number[] = [];
	const benchmark: number[] = [];
	for (let i = 0; i < 60; i++) {
		ev = Math.max(6, Math.min(94, ev + (eq() - 0.36) * 5));
		bv = Math.max(6, Math.min(94, bv + (eq() - 0.45) * 4));
		series.push(ev);
		benchmark.push(bv);
	}
	return { series, benchmark };
}

const { series: equitySeries, benchmark: equityBenchmark } = buildEquitySeries();

export const EQUITY: EquityData = {
	value: '$128,442.10',
	delta: '+$14,220.50',
	deltaTone: 'gain',
	series: equitySeries,
	benchmark: equityBenchmark,
	min: 0,
	max: 100
};

// --- Watchlist (reference lines 183-201, values at 3108-3111) ---

export interface WatchlistRow {
	ticker: string;
	venue: string;
	last: string;
	chg: string;
	tone: Tone;
}

export const WATCHLIST: WatchlistRow[] = SYMS.map((s) => ({
	ticker: s.ticker,
	venue: s.venue,
	last: fmt(s.base),
	chg: pct(s.chg),
	tone: toneOf(s.chg)
}));

// --- Positions (reference lines 203-217, values at 3112-3131) ---

export interface PositionRow {
	ticker: string;
	side: 'LONG' | 'SHORT';
	sideTone: Tone;
	entry: string;
	mark: string;
	pnl: string;
	tone: Tone;
}

interface PositionSeed {
	pair: string;
	side: 'LONG' | 'SHORT';
	qty: number;
	/** entry offset from mark, e.g. -0.031 = entry 3.1% below mark. */
	off: number;
}

const POSITION_SEEDS: PositionSeed[] = [
	{ pair: 'BTC/USDT', side: 'LONG', qty: 0.42, off: -0.031 },
	{ pair: 'SOL/USDT', side: 'LONG', qty: 74, off: -0.026 },
	{ pair: 'XAU/USD', side: 'SHORT', qty: 6, off: -0.004 },
	{ pair: 'ARB/USDT', side: 'SHORT', qty: 3200, off: 0.038 }
];

export const POSITIONS: PositionRow[] = POSITION_SEEDS.map((p) => {
	const sym = SYMS.find((s) => s.pair === p.pair) ?? SYMS[0];
	const mark = sym.base;
	const entry = mark * (1 + p.off);
	const dir = p.side === 'LONG' ? 1 : -1;
	const pnlv = (mark - entry) * p.qty * dir;
	return {
		ticker: sym.ticker,
		side: p.side,
		sideTone: p.side === 'LONG' ? 'gain' : 'loss',
		entry: fmt(entry),
		mark: fmt(mark),
		pnl: (pnlv >= 0 ? '+$' : '-$') + Math.abs(pnlv).toLocaleString('en-US', { maximumFractionDigits: 0 }),
		tone: toneOf(pnlv)
	};
});

// --- Risk meters (reference lines 219-234, values at 3129-3130) ---

export interface RiskBar {
	label: string;
	value: string;
	fraction: number;
	tone: Tone;
}

export const RISK_BARS: RiskBar[] = [
	{ label: 'PORTFOLIO HEAT', value: '92%', fraction: 0.92, tone: 'neutral' },
	{ label: 'MARGIN USED', value: '7.4%', fraction: 0.074, tone: 'neutral' },
	{ label: 'DRAWDOWN', value: '-2.1%', fraction: 0.21, tone: 'loss' },
	{ label: 'ALLOC · BTC', value: '41%', fraction: 0.41, tone: 'gain' }
];

// --- Volatility heatmap (reference lines 236-247, generator at 2227-2232) ---

const heatRng = mulberry(9);
export const HEAT_CELLS: number[] = Array.from({ length: 56 }, () => {
	const v = heatRng();
	return v > 0.86 ? 0.9 : v > 0.7 ? 0.55 : v > 0.5 ? 0.28 : v > 0.3 ? 0.14 : 0.06;
});
export const HEAT_MONTH_LABELS = ['MAY', 'JUN', 'JUL', 'AUG'];

// --- Signal desk (reference lines 249-266, values at 3132-3136) ---

export interface Signal {
	pair: string;
	verdict: string;
	tone: Tone;
	conf: string;
	note: string;
}

export const SIGNALS: Signal[] = [
	{
		pair: 'BTC/USDT',
		verdict: 'BULLISH',
		tone: 'gain',
		conf: '78%',
		note: 'Momentum expansion with rising spot inflow; 4H structure holds above 66.1K.'
	},
	{
		pair: 'XAU/USD',
		verdict: 'FADE',
		tone: 'loss',
		conf: '54%',
		note: 'Exhaustion wick into supply, DXY divergence building.'
	},
	{
		pair: 'SOL/USDT',
		verdict: 'ACCUMULATE',
		tone: 'gain',
		conf: '66%',
		note: 'Basis flat, funding neutral — range low is defended.'
	}
];

// --- Buy / sell flow (reference lines 268-282, generator at 2244) ---

export interface FlowBar {
	up: string;
	down: string;
}

const flowRng = mulberry(23);
export const FLOW_BARS: FlowBar[] = Array.from({ length: 10 }, () => ({
	up: (25 + flowRng() * 70).toFixed(0) + '%',
	down: (15 + flowRng() * 55).toFixed(0) + '%'
}));

export const FLOW_SUMMARY = { netBuy: '+$15,256', txns: '45,447', vol: '150M' };

// --- News tape (reference lines 284-293, values at 3138-3143) ---

export interface FeedItem {
	time: string;
	tag: string;
	tone: Tone;
	text: string;
}

export const FEED: FeedItem[] = [
	{ time: '17:32', tag: 'MACRO', tone: 'neutral', text: 'US 10Y yields slip 4bps ahead of PCE print.' },
	{
		time: '17:18',
		tag: 'FLOW',
		tone: 'gain',
		text: 'Spot BTC ETFs log $412M net inflow, third straight session.'
	},
	{
		time: '16:54',
		tag: 'CHAIN',
		tone: 'neutral',
		text: '18,400 ETH moved from CEX cold wallet to unknown address.'
	},
	{
		time: '16:31',
		tag: 'RISK',
		tone: 'loss',
		text: 'Perp open interest at 30-day high — liquidation cluster near 66.8K.'
	}
];
