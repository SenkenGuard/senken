// Local-only demo fixtures for the charts page's trade-engine furniture —
// the order ticket, the watchlist shortcut list, and the notes tab. Trade
// engine, backtesting, order routing ... [are] out of scope" for the whole
// plan, not just this milestone, and none of the owner's thirteen
// acceptance points §0) even mention an order ticket. There is no server
// surface for any of this — no `senken-execution`-shaped crate exists, no
// orders/positions endpoint is mounted, and none is planned.
//
// This file replaces the parts of `$lib/mock/charts.ts` that backed these
// widgets (that whole module is deleted —). The order-book ladder that used
// to live here (`genOrderBook`, a seeded-PRNG depth ladder around the pane's
// real last close) is gone: `crates/subscription::BookSource` now serves a
// real fixed-depth snapshot over the WS `book:<source>:<symbol>` topic, and
// `./order-book.ts`'s `bookRowsFromSnapshot` builds this panel's rows from
// that — a fallback ladder that merely *looked* real was exactly the
// mistake 019 M3 exists to close, so it is not kept as one.
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
