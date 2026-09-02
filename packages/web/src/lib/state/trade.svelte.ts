// The trade engine's client-side state: the registered adapters, the
// accounts this user has attached, which one is active, and — as of this
// plan — the one shared copy of every account's portfolio (balances,
// positions, orders, fills).
//
// A module-level rune store rather than Svelte context, for the same reason
// the command palette and AI panel use one: this is read from two routes
// (the engine page and the charts page's order ticket) that never share a
// component tree. Before this plan those two routes each kept their own
// `portfolios`/`orders` copy, which is exactly how they disagreed — an
// order placed from the chart never appeared on the engine page, and a
// resting order the simulator filled on its own stayed "working" on
// whichever screen did not cause the fill. One shared copy is what makes
// both screens agree, and it is the only way a fill neither screen caused
// can reach either of them.
//
// **Nothing here is a cache of convenience.** The broker behind an account
// is still the system of record; `portfolios` holds only what the adapter
// most recently answered, refreshed by `refreshPortfolios()`/`watchTrade()`
// below (see `lib/trade/portfolio-refresh.ts` and `lib/trade/poller.ts` for
// the two properties that make that refresh trustworthy: it always reads
// the account list as it stands *now*, and one account's failure never
// blanks another's already-fetched portfolio).

import { apiClient } from '$lib/api/client';
import { describeError } from '$lib/api/errors';
import { refreshPortfolios as runPortfolioRefresh } from '$lib/trade/portfolio-refresh';
import { TradePoller } from '$lib/trade/poller';
import { WatchScopes, type TradeWatchScope } from '$lib/trade/watch-scope';
import { emptyPortfolio, type Portfolio } from '$lib/trade/view';
import type { AccountAccessDto, AdapterDto, AdapterHealthDto, TradeAccountDto } from '$lib/api/types';

/** Where the account list has got to, so a screen can tell "no accounts
 * yet" from "not loaded yet" — the two need completely different copy. */
export type LoadState = 'idle' | 'loading' | 'ready' | 'error';

class TradeStore {
	adapters = $state<AdapterDto[]>([]);
	accounts = $state<TradeAccountDto[]>([]);
	/** This user's own accounts' resolved access, keyed by account id.
	 * Populated only for owned accounts — the server resolves access
	 * through the same owner-only path it resolves settings through, so an
	 * account belonging to someone else never has an entry here. Absence
	 * of an entry (not yet loaded, or the lookup failed) is read as
	 * unrestricted by every caller below rather than blocking on it. */
	access = $state<Record<string, AccountAccessDto>>({});
	/** This user's own accounts' resolved health, keyed by account id — the
	 * same owner-only, best-effort-absence rule `access` documents above,
	 * resolved through the very same `tradeAccountState` call. */
	health = $state<Record<string, AdapterHealthDto>>({});
	/** Every visible account's most recently fetched portfolio, keyed by
	 * account id. See this file's header comment for why this is shared
	 * rather than kept per-screen, and `refreshPortfolios`/`watchTrade`
	 * below for how it stays fresh. Absent for an account not yet fetched. */
	portfolios = $state<Record<string, Portfolio>>({});
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

/** Resolves access and health for every account this user owns and is
 * enabled — the only ones an order ticket would ever send to, or a health
 * badge would ever mean anything for — in the one round trip
 * `tradeAccountState` already makes for `access` alone (plan 001), rather
 * than adding a second one for `health`. Takes the account list **as
 * given**, never re-read from `tradeStore.accounts` itself: a caller
 * mid-reload has already replaced that with a newer list, and re-reading it
 * here would race whichever assignment runs last. One account's adapter
 * being unreachable must not blank every other account's badge, so a
 * failed lookup simply leaves that id absent from both maps rather than
 * failing the whole call. */
async function loadAccountState(accounts: TradeAccountDto[]): Promise<void> {
	const nextAccess: Record<string, AccountAccessDto> = {};
	const nextHealth: Record<string, AdapterHealthDto> = {};
	await Promise.all(
		accounts
			.filter((account) => account.owned && account.enabled)
			.map(async (account) => {
				try {
					const state = await apiClient.tradeAccountState(account.id);
					nextAccess[account.id] = state.access;
					nextHealth[account.id] = state.health;
				} catch {
					// Left absent — see this function's own docs.
				}
			})
	);
	tradeStore.access = nextAccess;
	tradeStore.health = nextHealth;
}

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
		await loadAccountState(accounts.rows);
	} catch (error) {
		tradeStore.error = describeError(error);
		tradeStore.load = 'error';
	}
}

/** Reloads just the accounts, after attaching, renaming, detaching one, or
 * changing its settings — the one place access or health can change. */
export async function refreshAccounts(): Promise<void> {
	const page = await apiClient.listTradeAccounts();
	tradeStore.accounts = page.rows;
	if (tradeStore.activeId && !page.rows.some((row) => row.id === tradeStore.activeId)) {
		tradeStore.activeId = null;
	}
	await loadAccountState(page.rows);
}

/** Makes `id` the account the order ticket sends to. */
export function selectAccount(id: string): void {
	tradeStore.activeId = id;
}

// ------------------------------------------------------------------------
// Portfolios: balances, positions, orders and fills, read through each
// account's adapter and shared by both the engine page and the chart
// panel. See this file's header comment and `lib/trade/portfolio-refresh.ts`
// for why the actual refresh logic lives in that plain module rather than
// here.
// ------------------------------------------------------------------------

/** Fetches one account's whole portfolio. Never rejects — a transport
 * failure is recorded on `Portfolio.error` and everything else is left at
 * its empty default, so one account's adapter being unreachable shows as a
 * message on that account's own card rather than throwing past its caller. */
async function fetchOnePortfolio(account: TradeAccountDto): Promise<Portfolio> {
	const portfolio = emptyPortfolio();
	try {
		const [balances, positions, orders, fills] = await Promise.all([
			apiClient.tradeAccountBalances(account.id),
			apiClient.tradeAccountPositions(account.id),
			apiClient.tradeAccountOrders(account.id, 'all'),
			apiClient.tradeAccountFills(account.id)
		]);
		portfolio.balances = balances;
		portfolio.positions = positions;
		portfolio.orders = orders;
		portfolio.fills = fills;
	} catch (error) {
		portfolio.error = describeError(error);
	}
	return portfolio;
}

/** Refreshes every account's portfolio, reading `tradeStore.accounts` as it
 * stands at the moment this is called — never a list captured earlier. This
 * is the one call both `watchTrade`'s timer and a screen's own REFRESH
 * button go through, and it is why an account attached or removed since the
 * last refresh is always handled correctly: `runPortfolioRefresh` (see
 * `lib/trade/portfolio-refresh.ts`) re-reads the list itself rather than
 * being handed one. */
export async function refreshPortfolios(): Promise<void> {
	await runPortfolioRefresh({
		listAccounts: () => tradeStore.accounts,
		listPortfolioIds: () => Object.keys(tradeStore.portfolios),
		fetchPortfolio: fetchOnePortfolio,
		write: (id, portfolio) => {
			tradeStore.portfolios[id] = portfolio;
		},
		remove: (id) => {
			delete tradeStore.portfolios[id];
		},
		describeError
	});
}

/** Refreshes one account's portfolio. If the account is no longer in
 * `tradeStore.accounts` (detached elsewhere between the mutation and this
 * call), its portfolio is dropped instead of being asked about. */
export async function refreshPortfolio(id: string): Promise<void> {
	const account = tradeStore.accounts.find((candidate) => candidate.id === id);
	if (!account) {
		delete tradeStore.portfolios[id];
		return;
	}
	tradeStore.portfolios[id] = await fetchOnePortfolio(account);
}

/** What every place/cancel/amend/close/action call awaits: refreshes just
 * the one account it acted on, immediately, rather than waiting for the
 * next auto-refresh tick or re-fetching every other account too. */
export async function afterMutation(id: string): Promise<void> {
	await refreshPortfolio(id);
}

/** What the current watchers between them need, counted in `WatchScopes`
 * (`lib/trade/watch-scope.ts`, where the rule is tested) rather than here —
 * this module cannot be imported under `bun test`. */
const watchScopes = new WatchScopes();

/** What the timer does each tick: the widest scope any current watcher
 * asked for, read fresh every time rather than fixed when the timer
 * started — a screen mounted or unmounted since then must change what the
 * next tick fetches. */
async function refreshWatched(): Promise<void> {
	if (watchScopes.current() === 'all') {
		await refreshPortfolios();
		return;
	}
	const active = tradeStore.active;
	if (active) await refreshPortfolio(active.id);
}

/** The auto-refresh timer's rules (interval, hidden/visible, no overlap,
 * survives a failed tick) live in `TradePoller` (`lib/trade/poller.ts`),
 * tested there against fake timers. This is the one instance, wired to the
 * real browser clock and to `refreshWatched` above; `watchTrade` below is
 * the only thing that ever touches it. */
const tradePoller = new TradePoller<ReturnType<typeof setInterval>>({
	intervalMs: 5_000,
	tick: refreshWatched,
	isHidden: () => document.hidden,
	setInterval: (handler, ms) => setInterval(handler, ms),
	clearInterval: (handle) => clearInterval(handle),
	onVisibilityChange: (handler) => {
		document.addEventListener('visibilitychange', handler);
		return () => document.removeEventListener('visibilitychange', handler);
	}
});

/** Starts trade auto-refresh at `scope`, returning the stop function.
 *
 * Reference counted, in the same shape as `senken_subscription::Lease`: two
 * mounted views share one timer, and it stops only once the last caller has
 * released. Call from `onMount` and return the result — Svelte calls it
 * again on destroy, which is the whole lifecycle:
 * `onMount(() => watchTrade('active'))`.
 *
 * Scope is per caller and the tick takes the widest one currently asked
 * for, so opening the engine page beside a chart widens the refresh and
 * closing it narrows it again, with no screen having to coordinate with any
 * other. */
export function watchTrade(scope: TradeWatchScope): () => void {
	const releaseScope = watchScopes.add(scope);
	const releasePoller = tradePoller.start();
	return () => {
		releaseScope();
		releasePoller();
	};
}
