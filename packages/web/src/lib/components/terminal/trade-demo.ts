// Local-only demo fixtures for the charts page's trade-engine furniture —
// the order ticket, its order book, the watchlist shortcut list, and the
// notes tab. Trade engine, backtesting,
// order routing ... [are] out of scope" for the whole plan, not just this
// milestone, and none of the owner's thirteen acceptance points
// §0) even mention an order ticket. There is no server surface for any of
// this — no `senken-execution`-shaped crate exists, no orders/positions
// endpoint is mounted, and none is planned.
//
// This file replaces the parts of `$lib/mock/charts.ts` that backed these
// widgets (that whole module is deleted —). The one thing
// deliberately different from before: `genOrderBook` now takes the pane's
// *real* last close (from `$lib/charts/bars.ts`, itself from
// `GET /api/bars/range`) instead of deriving a price from mock candles, so
// even a permanently-out-of-scope demo ladder is built around one real
// number rather than a second, disconnected fake series. The ladder's
// spread/depth around that price is still synthetic — there is no real
// order-book feed to show (that would be the live-feed leases of, itself scoped only to bars/indicator subscriptions, never a book).
import type { Component } from 'svelte';
import CpuIcon from '@lucide/svelte/icons/cpu';

// --- Watchlist ---------------------------------------------------------

export interface WatchlistRow {
	tag: string;
	ticker: string;
	venue: string;
	instrument: string;
}

// --- Notes ---------------------------------------------------------------

export const NOTE_TAGS = ['#BREAKOUT', '#LONDON', '#A-SETUP', '#RISK-1R'];
export const ACTIVE_NOTE = 'Notes are not saved yet.';

// --- Trade engine ticket -------------------------------------------------

export interface Account {
	id: string;
	label: string;
	mode: 'DEMO' | 'LIVE';
	ccy: string;
	leverage: string;
}

/** Out of scope (Part D) — a fixed simulated account, never a real one. */
export const DEFAULT_ACCOUNT: Account = { id: 'sim-1', label: 'SIM · GROWTH', mode: 'DEMO', ccy: 'USD', leverage: '1:10' };
export const ACCOUNT_ICON: Component = CpuIcon;
export const ACCOUNT_ADAPTER_LABEL = 'SENKEN SIMULATION · ALL INSTRUMENTS (DEMO, NOT WIRED)';

export type OrderSide = 'BUY' | 'SELL';
export type OrderType = 'MARKET' | 'LIMIT' | 'STOP';
export const ORDER_TYPES: OrderType[] = ['MARKET', 'LIMIT', 'STOP'];
export const SIZE_PRESETS = ['25%', '50%', '75%', 'MAX'];

export type OrderStatus = 'WORKING' | 'FILLED' | 'CANCELLED';

export interface OrderRow {
	id: string;
	ts: string;
	instrument: string;
	side: OrderSide;
	type: OrderType;
	qty: number;
	price: number;
	status: OrderStatus;
}

export const ENGINE_SUB_TABS: { key: 'book' | 'orders'; label: string }[] = [
	{ key: 'book', label: 'ORDER BOOK' },
	{ key: 'orders', label: 'ORDERS' }
];

export interface BookRow {
	price: string;
	size: string;
	side: 'ask' | 'bid';
	depthPct: number;
}

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

/** A synthetic depth ladder around `mid` — deterministic (seed 77) so it is
 * stable across re-renders. `mid` should be a real last-close price when
 * one is available; there is no real order-book feed behind this (see this
 * file's header comment). */
export function genOrderBook(mid: number): BookRow[] {
	if (!Number.isFinite(mid) || mid <= 0) return [];
	const rnd = mulberry(77);
	const fmt = (v: number) => (v >= 1000 ? v.toLocaleString('en-US', { maximumFractionDigits: 2, minimumFractionDigits: 2 }) : v >= 10 ? v.toFixed(2) : v.toFixed(4));
	const rows: BookRow[] = [];
	for (let i = 6; i >= 1; i--) {
		rows.push({ price: fmt(mid * (1 + i * 0.0006)), size: (rnd() * 4 + 0.2).toFixed(3), side: 'ask', depthPct: Math.round(20 + rnd() * 60) });
	}
	for (let i = 1; i <= 6; i++) {
		rows.push({ price: fmt(mid * (1 - i * 0.0006)), size: (rnd() * 4 + 0.2).toFixed(3), side: 'bid', depthPct: Math.round(20 + rnd() * 60) });
	}
	return rows;
}
