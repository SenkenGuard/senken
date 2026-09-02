// The trade engine's client-side state: the registered adapters, the
// accounts this user has attached, and which one is active.
//
// A module-level rune store rather than Svelte context, for the same reason
// the command palette and AI panel use one: this is read from two routes
// (the engine page and the charts page's order ticket) that never share a
// component tree.
//
// **Nothing here caches a portfolio.** Balances, positions, orders and
// fills are read through the adapter on every request, because the broker
// behind an account is the system of record for them and a copy here could
// only ever be a copy that disagrees. What this store holds is the
// adapter/account *catalogue* and the current selection — facts Senken
// itself owns.

import { apiClient } from '$lib/api/client';
import { describeError } from '$lib/api/errors';
import type { AdapterDto, TradeAccountDto } from '$lib/api/types';

/** Where the account list has got to, so a screen can tell "no accounts
 * yet" from "not loaded yet" — the two need completely different copy. */
export type LoadState = 'idle' | 'loading' | 'ready' | 'error';

class TradeStore {
	adapters = $state<AdapterDto[]>([]);
	accounts = $state<TradeAccountDto[]>([]);
	activeId = $state<string | null>(null);
	load = $state<LoadState>('idle');
	error = $state<string | null>(null);

	/** The adapter behind `adapterId`, or `undefined` when a plugin that
	 * once registered it is no longer installed — which is a real state: an
	 * account row outlives the plugin that could trade it. */
	adapter(adapterId: string): AdapterDto | undefined {
		return this.adapters.find((adapter) => adapter.id === adapterId);
	}

	/** The accounts this user owns — the only ones they can trade with or
	 * read settings for. An operator may see others in the list. */
	get ownAccounts(): TradeAccountDto[] {
		return this.accounts.filter((account) => account.owned);
	}

	/** The account an order ticket would send to: the selected one when it
	 * is still there, otherwise the first the user owns and has enabled. */
	get active(): TradeAccountDto | undefined {
		const selected = this.accounts.find((account) => account.id === this.activeId);
		if (selected) return selected;
		return this.ownAccounts.find((account) => account.enabled) ?? this.ownAccounts[0];
	}
}

export const tradeStore = new TradeStore();

/** Loads the adapters and accounts. Safe to call again — a screen that
 * mounts twice refreshes rather than duplicating anything. */
export async function loadTrade(): Promise<void> {
	tradeStore.load = tradeStore.load === 'ready' ? 'ready' : 'loading';
	tradeStore.error = null;
	try {
		const [adapters, accounts] = await Promise.all([
			apiClient.listTradeAdapters(),
			apiClient.listTradeAccounts()
		]);
		tradeStore.adapters = adapters.adapters;
		tradeStore.accounts = accounts.rows;
		// Derive the selection from the list that just arrived, never from
		// the id the screen opened with: an account deleted elsewhere would
		// otherwise stay selected and every panel would keep asking a
		// server about a row that no longer exists.
		if (tradeStore.activeId && !accounts.rows.some((row) => row.id === tradeStore.activeId)) {
			tradeStore.activeId = null;
		}
		tradeStore.load = 'ready';
	} catch (error) {
		tradeStore.error = describeError(error);
		tradeStore.load = 'error';
	}
}

/** Reloads just the accounts, after attaching, renaming or detaching one. */
export async function refreshAccounts(): Promise<void> {
	const page = await apiClient.listTradeAccounts();
	tradeStore.accounts = page.rows;
	if (tradeStore.activeId && !page.rows.some((row) => row.id === tradeStore.activeId)) {
		tradeStore.activeId = null;
	}
}

/** Makes `id` the account the order ticket sends to. */
export function selectAccount(id: string): void {
	tradeStore.activeId = id;
}
