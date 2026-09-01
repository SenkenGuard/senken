// Local-only demo fixtures for the charts page's trade-engine furniture —
// the order ticket and its order list. Trading is out of scope: there is no
// server surface for any of it — no execution crate exists, no
// orders/positions endpoint is mounted, and nothing here reaches a broker.
// The panel says so on screen (`ENGINE_DEMO_NOTICE`) rather than leaving a
// ticket that looks like it trades.
//
// The order-book ladder that used to live here (`genOrderBook`, a
// seeded-PRNG depth ladder around the pane's real last close) is gone:
// `crates/subscription::BookSource` serves a real fixed-depth snapshot over
// the WS `book:<source>:<symbol>` topic, and `./order-book.ts`'s
// `bookRowsFromSnapshot` builds that panel's rows from it. A fallback
// ladder that merely *looked* real is worse than none, so none is kept.
import type { Component } from 'svelte';
import CpuIcon from '@lucide/svelte/icons/cpu';

// --- Watchlist ---------------------------------------------------------

export interface WatchlistRow {
	tag: string;
	ticker: string;
	venue: string;
	instrument: string;
}

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

/** The panel's permanent, unavoidable disclosure that the ticket above it
 * trades nothing real — the order goes into a local array and reaches no
 * broker. Leaving that implied rather than said is the one thing this
 * feature must not do. */
export const ENGINE_DEMO_NOTICE = 'Demo only — no broker is connected and no order leaves this screen.';

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
