// Mock data for the shared shell chrome (NavRail / FooterBar).
//
// Ported from the design reference — NAV at
// line 1285, NAV_WIDGETS at line 1321, FOOT_WIDGETS at line 1331, SYMS at
// line 1267, footerSegs' detail rows at lines 3002-3031. No `fetch`, no
// `/api` calls: this page is UI-only, so every value here is a fixture, not
// a snapshot of anything real.

import type { Component } from 'svelte';
import LayoutDashboardIcon from '@lucide/svelte/icons/layout-dashboard';
import TrendingUpIcon from '@lucide/svelte/icons/trending-up';
import ZapIcon from '@lucide/svelte/icons/zap';
import PackageSearchIcon from '@lucide/svelte/icons/package-search';
import ActivityIcon from '@lucide/svelte/icons/activity';
import TerminalIcon from '@lucide/svelte/icons/terminal';
import BellIcon from '@lucide/svelte/icons/bell';
import ClockIcon from '@lucide/svelte/icons/clock';
import ShieldCheckIcon from '@lucide/svelte/icons/shield-check';

/** The reference's three routes, plus two the reference never had and so
 * has no line to port from: `workspaces` for the real, server-backed
 * dashboard at `/dashboard` (`routes/dashboard/+page.svelte`), and
 * `registry` for the indicator registry at `/registry`
 * (`routes/registry/+page.svelte`). `href` is real SvelteKit routing, not
 * the reference's `page` state variable. */
export interface NavEntry {
	key: 'dashboard' | 'workspaces' | 'charts' | 'engine' | 'registry';
	href: string;
	icon: Component;
	label: string;
	desc: string;
}

export const NAV: NavEntry[] = [
	{
		key: 'dashboard',
		href: '/dashboard',
		icon: LayoutDashboardIcon,
		label: 'DASHBOARD',
		desc: 'Workspaces, widgets, portfolio shortcuts'
	},
	{
		key: 'charts',
		href: '/charts',
		icon: TrendingUpIcon,
		label: 'CHARTS',
		desc: 'Multi-window charting, drawings, replay'
	},
	{
		key: 'engine',
		href: '/engine',
		icon: ZapIcon,
		label: 'TRADE ENGINE',
		desc: 'Order routing, risk caps, execution log'
	},
	{
		key: 'registry',
		href: '/registry',
		icon: PackageSearchIcon,
		label: 'REGISTRY',
		desc: 'Search, read, fork, and install published indicators'
	}
];

/** One key/value row inside a footer segment's hover detail card. `tone`
 * mirrors the reference's gain/loss/neutral coloring. */
export interface FooterDetailRow {
	k: string;
	v: string;
	tone: 'gain' | 'loss' | 'neutral';
}

export interface FooterWidget {
	key: string;
	label: string;
	icon: Component;
	short: string;
	details: FooterDetailRow[];
}

export const FOOT_WIDGETS: FooterWidget[] = [
	{
		key: 'status',
		label: 'SYSTEM HEALTH',
		icon: ActivityIcon,
		short: 'SYS OK',
		details: [
			{ k: 'STATUS', v: 'ALL SYSTEMS OK', tone: 'gain' },
			{ k: 'CPU / MEM', v: '12% · 486 MB', tone: 'neutral' },
			{ k: 'WS LATENCY', v: '31 ms', tone: 'neutral' },
			{ k: 'DATA FEEDS', v: '4 / 4 CONNECTED', tone: 'gain' }
		]
	},
	{
		key: 'log',
		label: 'EXECUTION LOG',
		icon: TerminalIcon,
		short: 'LOG',
		details: [
			{ k: '17:33:12', v: 'FILL BTCUSDT 0.25 @ 68,420.00', tone: 'neutral' },
			{ k: '17:31:48', v: 'ORDER SENT · LIMIT', tone: 'neutral' },
			{ k: '17:28:04', v: 'LAYER ADDED · EMA 200', tone: 'neutral' },
			{ k: '17:26:11', v: 'WORKSPACE SAVED', tone: 'neutral' }
		]
	},
	{
		key: 'ticker',
		label: 'MARKET TAPE',
		icon: ActivityIcon,
		short: 'TAPE',
		details: []
	},
	{
		key: 'notif',
		label: 'NOTIFICATIONS',
		icon: BellIcon,
		short: '3',
		details: [
			{ k: 'SOLUSDT', v: 'TRIGGERED · 176.40', tone: 'loss' },
			{ k: 'BTCUSDT', v: 'ARMED · 68,800', tone: 'gain' },
			{ k: 'XAUUSD', v: 'ARMED · RSI 72', tone: 'gain' }
		]
	},
	{
		key: 'session',
		label: 'SESSION CLOCK',
		icon: ClockIcon,
		short: 'LDN',
		details: [
			{ k: 'ACTIVE', v: 'LONDON', tone: 'neutral' },
			{ k: 'LOCAL', v: '00:00:00', tone: 'neutral' },
			{ k: 'NEXT', v: 'NEW YORK · 2H 12M', tone: 'neutral' }
		]
	},
	{
		key: 'risk',
		label: 'RISK SUMMARY',
		icon: ShieldCheckIcon,
		short: '2.1R',
		details: [
			{ k: 'OPEN RISK', v: '2.1R', tone: 'neutral' },
			{ k: 'POSITIONS', v: '4', tone: 'neutral' },
			{ k: 'DAILY LIMIT', v: '3.0R', tone: 'neutral' }
		]
	}
];

/** Default visible footer segments (reference: `footWidgets` initial
 * state). Order follows `FOOT_WIDGETS`, matching the reference's `.filter`. */
export const DEFAULT_FOOT_KEYS = ['status', 'log', 'ticker', 'notif'];

/** Market tape row (reference: `tickerItems`, fed from `SYMS`). Prices are
 * each symbol's fixture base price, not a simulated candle series — bar
 * simulation belongs to the charts page, not the shell. */
export interface TickerRow {
	pair: string;
	last: string;
	chg: string;
	positive: boolean;
}

interface Sym {
	pair: string;
	venue: string;
	ticker: string;
	base: number;
	chg: number;
}

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

export const TICKER_ITEMS: TickerRow[] = SYMS.map((s) => ({
	pair: `${s.venue}:${s.ticker}`,
	last: fmt(s.base),
	chg: pct(s.chg),
	positive: s.chg >= 0
}));
