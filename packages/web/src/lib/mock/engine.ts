// Mock data for the trade engine page.
//
// Ported from the same design reference as shell.ts/dashboard.ts: `ADAPTERS`
// at line 1291, the initial `accounts`/`orders` state at lines 269-282, the
// `POS_SEED` fixture and its per-account derivation (`posOf`) around lines
// 1494-1512, and the `aggStats` / `exposure` / `accountCards` / `acctStats`
// / `adapters` / table derived props at lines ~1611-1719.
//
// No `fetch`, no `/api` calls: this page is UI-only, so every value here is a
// fixture, not a snapshot of anything real.
//
// Deviation from the reference: `lastPrice()` there comes from `genCandles`,
// a seeded OHLC simulator that belongs to the charts page (which owns
// `chart-pane` and candle simulation — `dashboard.ts` documents the same
// call). Duplicating that simulator here would cross the P3/P4 file
// boundary, so positions use each symbol's fixture `base` price directly
// instead — the same number `genCandles` would settle near, without
// importing its code.

import type { Component } from 'svelte';
import CpuIcon from '@lucide/svelte/icons/cpu';
import PlugZapIcon from '@lucide/svelte/icons/plug-zap';
import CoinsIcon from '@lucide/svelte/icons/coins';
import TrendingUpIcon from '@lucide/svelte/icons/trending-up';
import LandmarkIcon from '@lucide/svelte/icons/landmark';
import PlugIcon from '@lucide/svelte/icons/plug';
import LayoutGridIcon from '@lucide/svelte/icons/layout-grid';
import WalletIcon from '@lucide/svelte/icons/wallet';

/** One symbol fixture (reference: `SYMS`, line 1267) — only the fields the
 * engine page needs. Full OHLC/tag/name/chg fields belong to the charts
 * and shell fixtures. */
interface Sym {
	pair: string;
	venue: string;
	ticker: string;
	base: number;
}

const SYMS: Sym[] = [
	{ pair: 'BTC/USDT', venue: 'BINANCE', ticker: 'BTCUSDT', base: 68420 },
	{ pair: 'ETH/USDT', venue: 'BINANCE', ticker: 'ETHUSDT', base: 3512 },
	{ pair: 'SOL/USDT', venue: 'BINANCE', ticker: 'SOLUSDT', base: 182.4 },
	{ pair: 'XAU/USD', venue: 'OANDA', ticker: 'XAUUSD', base: 2412 },
	{ pair: 'EUR/USD', venue: 'OANDA', ticker: 'EURUSD', base: 1.0864 },
	{ pair: 'SUI/USDT', venue: 'BYBIT', ticker: 'SUIUSDT', base: 7.552 },
	{ pair: 'NAS100', venue: 'OANDA', ticker: 'NAS100USD', base: 19842 },
	{ pair: 'ARB/USDT', venue: 'BINANCE', ticker: 'ARBUSDT', base: 1.142 }
];

function symOf(pair: string): Sym {
	return SYMS.find((s) => s.pair === pair) ?? SYMS[0];
}

/** "VENUE:TICKER", e.g. "BINANCE:BTCUSDT" (reference: `fullId`). */
function fullId(pair: string): string {
	const s = symOf(pair);
	return `${s.venue}:${s.ticker}`;
}

function lastPrice(pair: string): number {
	return symOf(pair).base;
}

export type AdapterStatus = 'CONNECTED' | 'AVAILABLE' | 'BETA';

export interface Adapter {
	key: string;
	name: string;
	kind: 'SIMULATION' | 'BROKER' | 'EXCHANGE';
	icon: Component;
	status: AdapterStatus;
	symbols: string[] | '*';
	note: string;
}

export const ADAPTERS: Adapter[] = [
	{
		key: 'sim',
		name: 'Senken Simulation',
		kind: 'SIMULATION',
		icon: CpuIcon,
		status: 'CONNECTED',
		symbols: '*',
		note: 'Runs in the app layer — every instrument on the platform is tradable.'
	},
	{
		key: 'mt5',
		name: 'MetaTrader 5 Bridge',
		kind: 'BROKER',
		icon: PlugZapIcon,
		status: 'CONNECTED',
		symbols: ['XAU/USD', 'EUR/USD', 'NAS100'],
		note: 'Local bridge · order, position and history sync.'
	},
	{
		key: 'binance',
		name: 'Binance Spot',
		kind: 'EXCHANGE',
		icon: CoinsIcon,
		status: 'CONNECTED',
		symbols: ['BTC/USDT', 'ETH/USDT', 'SOL/USDT', 'ARB/USDT'],
		note: 'REST + websocket keys, spot only.'
	},
	{
		key: 'bybit',
		name: 'Bybit Derivatives',
		kind: 'EXCHANGE',
		icon: TrendingUpIcon,
		status: 'AVAILABLE',
		symbols: ['BTC/USDT', 'ETH/USDT', 'SUI/USDT'],
		note: 'Perpetuals · requires API key with trade scope.'
	},
	{
		key: 'oanda',
		name: 'OANDA v20',
		kind: 'BROKER',
		icon: LandmarkIcon,
		status: 'AVAILABLE',
		symbols: ['XAU/USD', 'EUR/USD', 'NAS100'],
		note: 'FX and CFD via v20 REST.'
	},
	{
		key: 'ctrader',
		name: 'cTrader Open API',
		kind: 'BROKER',
		icon: PlugIcon,
		status: 'BETA',
		symbols: ['EUR/USD', 'XAU/USD'],
		note: 'OAuth flow, partial position sync.'
	}
];

export function adapterOf(key: string): Adapter {
	return ADAPTERS.find((a) => a.key === key) ?? ADAPTERS[0];
}

function adapterSupports(key: string, pair: string): boolean {
	const a = adapterOf(key);
	return a.symbols === '*' || a.symbols.includes(pair);
}

export type AccountMode = 'DEMO' | 'LIVE';

export interface Account {
	id: string;
	adapter: string;
	label: string;
	mode: AccountMode;
	ccy: string;
	balance: number;
	equity: number;
	leverage: string;
	margin: number;
}

export const ACCOUNTS: Account[] = [
	{
		id: 'sim-1',
		adapter: 'sim',
		label: 'SIM · GROWTH',
		mode: 'DEMO',
		ccy: 'USD',
		balance: 128442.1,
		equity: 130120.44,
		leverage: '1:10',
		margin: 9420
	},
	{
		id: 'mt5-1',
		adapter: 'mt5',
		label: 'MT5 · 8823941',
		mode: 'LIVE',
		ccy: 'USD',
		balance: 41220.55,
		equity: 41892.1,
		leverage: '1:100',
		margin: 3110
	},
	{
		id: 'bin-1',
		adapter: 'binance',
		label: 'BINANCE · MAIN',
		mode: 'LIVE',
		ccy: 'USDT',
		balance: 86110.2,
		equity: 86110.2,
		leverage: 'SPOT',
		margin: 0
	}
];

export const DEFAULT_ACCOUNT_ID = 'sim-1';

export type OrderSide = 'BUY' | 'SELL';
export type OrderKind = 'MARKET' | 'LIMIT' | 'STOP';
export type OrderStatus = 'WORKING' | 'FILLED' | 'CANCELLED';

export interface Order {
	id: string;
	ts: string;
	account: string;
	pair: string;
	side: OrderSide;
	type: OrderKind;
	qty: number;
	price: number;
	status: OrderStatus;
}

export const ORDERS: Order[] = [
	{ id: 'O-1041', ts: '17:31:48', account: 'sim-1', pair: 'BTC/USDT', side: 'BUY', type: 'LIMIT', qty: 0.25, price: 68120, status: 'WORKING' },
	{ id: 'O-1039', ts: '17:12:04', account: 'sim-1', pair: 'SOL/USDT', side: 'BUY', type: 'MARKET', qty: 74, price: 176.4, status: 'FILLED' },
	{ id: 'O-1036', ts: '16:48:22', account: 'mt5-1', pair: 'XAU/USD', side: 'SELL', type: 'LIMIT', qty: 6, price: 2398, status: 'FILLED' },
	{ id: 'O-1034', ts: '16:20:11', account: 'sim-1', pair: 'ETH/USDT', side: 'SELL', type: 'STOP', qty: 4, price: 3390, status: 'CANCELLED' }
];

interface PosSeed {
	pair: string;
	side: 'LONG' | 'SHORT';
	qty: number;
	off: number;
}

const POS_SEED: PosSeed[] = [
	{ pair: 'BTC/USDT', side: 'LONG', qty: 0.42, off: -0.031 },
	{ pair: 'SOL/USDT', side: 'LONG', qty: 74, off: -0.026 },
	{ pair: 'XAU/USD', side: 'SHORT', qty: 6, off: -0.004 },
	{ pair: 'EUR/USD', side: 'LONG', qty: 40000, off: -0.002 },
	{ pair: 'ETH/USDT', side: 'LONG', qty: 4.2, off: -0.018 },
	{ pair: 'NAS100', side: 'SHORT', qty: 3, off: 0.006 }
];

export interface Position {
	pair: string;
	side: 'LONG' | 'SHORT';
	qty: number;
	entry: number;
	mark: number;
	pnl: number;
	account: Account;
}

/** Positions open on one account (reference: `posOf`). `index` is the
 * account's position within `ACCOUNTS` — it feeds the same "every other
 * seed position" stagger the reference uses so demo accounts don't all
 * show an identical book. Non-sim adapters only ever see positions in
 * pairs they support (reference: `adapterSupports`). */
export function positionsForAccount(account: Account, index: number): Position[] {
	return POS_SEED.filter((p) => adapterSupports(account.adapter, p.pair))
		.filter((_p, i) => (i + index) % 2 === 0 || account.adapter === 'sim')
		.map((p) => {
			const mark = lastPrice(p.pair);
			const entry = mark * (1 + p.off);
			const dir = p.side === 'LONG' ? 1 : -1;
			return { pair: p.pair, side: p.side, qty: p.qty, entry, mark, pnl: (mark - entry) * p.qty * dir, account };
		});
}

/** Every account's positions, concatenated (reference: `allPositions`).
 * `accounts` defaults to the static fixture; the engine page passes the
 * reactive list from `$lib/state/accounts.svelte` so newly attached
 * accounts (the "ATTACH ACCOUNT") show up here too. */
export function allPositions(accounts: Account[] = ACCOUNTS): Position[] {
	return accounts.reduce<Position[]>((out, a, i) => out.concat(positionsForAccount(a, i)), []);
}

export function fmt(v: number): string {
	if (v >= 1000) return v.toLocaleString('en-US', { maximumFractionDigits: 2, minimumFractionDigits: 2 });
	if (v >= 10) return v.toFixed(2);
	return v.toFixed(4);
}

/** "+$1,234" / "-$1,234" (reference: the repeated
 * `(v >= 0 ? '+$' : '-$') + Math.abs(v).toLocaleString(..., { maximumFractionDigits: 0 })`
 * pattern used for every PNL figure). */
export function signedUsd(v: number): string {
	return (v >= 0 ? '+$' : '-$') + Math.abs(v).toLocaleString('en-US', { maximumFractionDigits: 0 });
}

/** Text-color tone keys, mapped to Tailwind classes by the components that
 * render them (reference: the inline `var(--fg)` / `var(--dim2)` / `cc(v)`
 * colors scattered through the derived engine props). */
export type Tone = 'fg' | 'fg2' | 'dim' | 'dim2' | 'gain' | 'loss' | 'inv';

export function ccTone(v: number): 'gain' | 'loss' {
	return v >= 0 ? 'gain' : 'loss';
}

export interface StatItem {
	label: string;
	value: string;
	tone: Tone;
}

/** Overview view's top stat strip (reference: `aggStats`). */
export function aggStats(accounts: Account[] = ACCOUNTS): StatItem[] {
	const eq = accounts.reduce((t, a) => t + a.equity, 0);
	const bal = accounts.reduce((t, a) => t + a.balance, 0);
	const positions = allPositions(accounts);
	const pnl = positions.reduce((t, p) => t + p.pnl, 0);
	return [
		{ label: 'TOTAL EQUITY', value: '$' + fmt(eq), tone: 'fg' },
		{ label: 'TOTAL BALANCE', value: '$' + fmt(bal), tone: 'fg' },
		{ label: 'OPEN PNL', value: signedUsd(pnl), tone: ccTone(pnl) },
		{ label: 'POSITIONS', value: String(positions.length), tone: 'fg2' },
		{ label: 'WORKING ORDERS', value: String(ORDERS.filter((o) => o.status === 'WORKING').length), tone: 'fg2' }
	];
}

/** Accounts view's per-account stat strip (reference: `acctStats`). */
export function acctStats(account: Account): StatItem[] {
	const dayPnl = account.equity - account.balance;
	return [
		{ label: 'BALANCE', value: (account.ccy === 'USD' ? '$' : '') + fmt(account.balance), tone: 'fg' },
		{ label: 'EQUITY', value: (account.ccy === 'USD' ? '$' : '') + fmt(account.equity), tone: 'fg' },
		{ label: 'DAY PNL', value: signedUsd(dayPnl), tone: ccTone(dayPnl) },
		{
			label: 'MARGIN USED',
			value: account.margin ? '$' + account.margin.toLocaleString('en-US') : '—',
			tone: 'fg2'
		}
	];
}

export interface ExposureItem {
	icon: Component;
	name: string;
	equity: string;
	pct: number;
	highlighted: boolean;
}

/** "Equity by adapter" bars (reference: `exposure`). The highlight flag is
 * fixed to the adapter of the *first* account in `ACCOUNTS`, computed
 * before the list is sorted by share — reproduced exactly as written, so it
 * lands on whichever adapter happens to own the first-opened account, not
 * necessarily the largest bar post-sort. */
export function exposure(accounts: Account[] = ACCOUNTS): ExposureItem[] {
	const total = accounts.reduce((t, a) => t + a.equity, 0) || 1;
	const keys = Array.from(new Set(accounts.map((a) => a.adapter)));
	const highlightKey = keys[0];
	return keys
		.map((k) => {
			const ad = adapterOf(k);
			const eq = accounts.filter((a) => a.adapter === k).reduce((t, a) => t + a.equity, 0);
			return {
				icon: ad.icon,
				name: ad.name,
				equity: '$' + fmt(eq),
				pct: Math.round((eq / total) * 100),
				highlighted: k === highlightKey
			};
		})
		.sort((a, b) => b.pct - a.pct);
}

export interface AccountCardData {
	id: string;
	label: string;
	mode: AccountMode;
	icon: Component;
	adapterName: string;
	equity: string;
	pnl: string;
	pnlTone: 'gain' | 'loss';
	selected: boolean;
}

/** Accounts view's card strip (reference: `accountCards`). */
export function accountCards(activeAccountId: string, accounts: Account[] = ACCOUNTS): AccountCardData[] {
	return accounts.map((a, i) => {
		const pnl = positionsForAccount(a, i).reduce((t, p) => t + p.pnl, 0);
		return {
			id: a.id,
			label: a.label,
			mode: a.mode,
			icon: adapterOf(a.adapter).icon,
			adapterName: adapterOf(a.adapter).name.toUpperCase(),
			equity: (a.ccy === 'USD' ? '$' : '') + fmt(a.equity),
			pnl: signedUsd(pnl),
			pnlTone: ccTone(pnl),
			selected: a.id === activeAccountId
		};
	});
}

export interface AdapterAccountRow {
	id: string;
	label: string;
	equity: string;
	selected: boolean;
	dotTone: 'gain' | 'loss';
}

export interface AdapterCardData {
	key: string;
	icon: Component;
	name: string;
	kind: string;
	status: AdapterStatus;
	note: string;
	connected: boolean;
	coverage: string;
	symbolChips: string[];
	attachLabel: string;
	accounts: AdapterAccountRow[];
}

/** Adapters view's card grid (reference: `adapters`). `accounts` defaults to
 * the static fixture — see `allPositions`'s comment above. */
export function adapterCards(activeAccountId: string, accounts: Account[] = ACCOUNTS): AdapterCardData[] {
	return ADAPTERS.map((a) => {
		const connected = a.status === 'CONNECTED';
		return {
			key: a.key,
			icon: a.icon,
			name: a.name,
			kind: a.kind,
			status: a.status,
			note: a.note,
			connected,
			coverage: a.symbols === '*' ? 'ALL INSTRUMENTS' : `${a.symbols.length} INSTRUMENTS`,
			symbolChips: a.symbols === '*' ? ['EVERY INSTRUMENT ON SENKEN'] : a.symbols.map((p) => symOf(p).ticker),
			attachLabel: connected ? 'ATTACH ACCOUNT' : 'CONNECT ADAPTER',
			accounts: accounts
				.filter((x) => x.adapter === a.key)
				.map((x) => ({
					id: x.id,
					label: x.label,
					equity: (x.ccy === 'USD' ? '$' : '') + fmt(x.equity),
					selected: x.id === activeAccountId,
					dotTone: x.mode === 'LIVE' ? 'loss' : 'gain'
				}))
		};
	});
}

export type EngineTableTab = 'positions' | 'orders' | 'exec';

export interface TableColumn {
	label: string;
	align: 'left' | 'right';
}

export interface TableCell {
	value: string;
	tone: Tone;
	align: 'left' | 'right';
}

export interface TableRow {
	cells: TableCell[];
}

export interface TableTabDef {
	key: EngineTableTab;
	label: string;
	count: number;
}

export interface EngineTable {
	tabs: TableTabDef[];
	columns: TableColumn[];
	rows: TableRow[];
	emptyLabel: string;
}

function accountLabelOf(accounts: Account[], id: string): string {
	return accounts.find((a) => a.id === id)?.label ?? id;
}

/** Builds the POSITIONS / ORDERS / EXECUTIONS table (reference: the
 * `tableCols` / `tableHead` / `tableRows` / `tableEmptyLabel` block,
 * lines ~1530-1566). `scope` mirrors the reference's `accCol` (there,
 * `isOverview`): 'all' shows every account's rows plus an ACCOUNT column,
 * 'account' scopes to one account and drops it — the two empty-state
 * labels ("...ACROSS ACCOUNTS" vs "...ON THIS ACCOUNT") come from the same
 * branch. `positions` and `orders` must already be scoped by the caller
 * (all of them, or just the active account's). */
export function engineTable(
	tab: EngineTableTab,
	scope: 'all' | 'account',
	positions: Position[],
	orders: Order[],
	allAccounts: Account[]
): EngineTable {
	const accCol = scope === 'all';
	const filledOrders = orders.filter((o) => o.status === 'FILLED');
	const tabs: TableTabDef[] = [
		{ key: 'positions', label: 'POSITIONS', count: positions.length },
		{ key: 'orders', label: 'ORDERS', count: orders.length },
		{ key: 'exec', label: 'EXECUTIONS', count: filledOrders.length }
	];

	const L = (value: string, tone: Tone = 'fg2'): TableCell => ({ value, tone, align: 'left' });
	const R = (value: string, tone: Tone = 'fg2'): TableCell => ({ value, tone, align: 'right' });
	const accCell = (id: string): TableCell[] => (accCol ? [L(accountLabelOf(allAccounts, id), 'dim2')] : []);
	const accHead: TableColumn[] = accCol ? [{ label: 'ACCOUNT', align: 'left' }] : [];

	if (tab === 'orders') {
		return {
			tabs,
			columns: [
				{ label: 'TIME', align: 'left' },
				{ label: 'INSTRUMENT', align: 'left' },
				...accHead,
				{ label: 'SIDE', align: 'left' },
				{ label: 'TYPE', align: 'left' },
				{ label: 'QTY', align: 'right' },
				{ label: 'PRICE', align: 'right' },
				{ label: 'STATUS', align: 'right' }
			],
			rows: orders.map((o) => ({
				cells: [
					L(o.ts, 'dim2'),
					L(fullId(o.pair), 'fg'),
					...accCell(o.account),
					L(o.side, o.side === 'BUY' ? 'gain' : 'loss'),
					L(o.type, 'dim2'),
					R(String(o.qty)),
					R(fmt(o.price)),
					R(o.status, o.status === 'FILLED' ? 'gain' : o.status === 'WORKING' ? 'fg' : 'dim')
				]
			})),
			emptyLabel: accCol
				? 'NO ORDERS ACROSS ACCOUNTS — PLACE ONE FROM THE CHART'
				: 'NO ORDERS ON THIS ACCOUNT — PLACE ONE FROM THE CHART'
		};
	}

	if (tab === 'exec') {
		return {
			tabs,
			columns: [
				{ label: 'TIME', align: 'left' },
				{ label: 'INSTRUMENT', align: 'left' },
				...accHead,
				{ label: 'SIDE', align: 'left' },
				{ label: 'QTY', align: 'right' },
				{ label: 'FILL', align: 'right' },
				{ label: 'FEE', align: 'right' }
			],
			rows: filledOrders.map((o) => ({
				cells: [
					L(o.ts, 'dim2'),
					L(fullId(o.pair), 'fg'),
					...accCell(o.account),
					L(o.side, o.side === 'BUY' ? 'gain' : 'loss'),
					R(String(o.qty)),
					R(fmt(o.price)),
					R('$' + (o.qty * o.price * 0.0004).toFixed(2), 'dim2')
				]
			})),
			emptyLabel: accCol ? 'NO EXECUTIONS ACROSS ACCOUNTS' : 'NO EXECUTIONS ON THIS ACCOUNT'
		};
	}

	return {
		tabs,
		columns: [
			{ label: 'INSTRUMENT', align: 'left' },
			...accHead,
			{ label: 'SIDE', align: 'left' },
			{ label: 'QTY', align: 'right' },
			{ label: 'ENTRY', align: 'right' },
			{ label: 'MARK', align: 'right' },
			{ label: 'PNL', align: 'right' }
		],
		rows: positions.map((p) => ({
			cells: [
				L(fullId(p.pair), 'fg'),
				...accCell(p.account.id),
				L(p.side, p.side === 'LONG' ? 'gain' : 'loss'),
				R(String(p.qty)),
				R(fmt(p.entry), 'dim2'),
				R(fmt(p.mark)),
				R(signedUsd(p.pnl), ccTone(p.pnl))
			]
		})),
		emptyLabel: accCol ? 'NO OPEN POSITIONS ACROSS ACCOUNTS' : 'NO OPEN POSITIONS ON THIS ACCOUNT'
	};
}

export interface EngineView {
	key: 'overview' | 'accounts' | 'adapters';
	label: string;
	icon: Component;
}

/** The three engine views (reference: the `engineViews` tab bar). */
export const ENGINE_VIEWS: EngineView[] = [
	{ key: 'overview', label: 'OVERVIEW', icon: LayoutGridIcon },
	{ key: 'accounts', label: 'ACCOUNTS', icon: WalletIcon },
	{ key: 'adapters', label: 'ADAPTERS', icon: PlugIcon }
];
