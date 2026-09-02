// The one property `refreshPortfolios` exists to guarantee: it reads the
// account list *as it stands when the call runs*, never a list captured
// earlier. This is deliberately its own plain, rune-free module — the same
// split `lib/api/session-funnel.ts` uses next to `session.svelte.ts` — for
// two reasons. First, testability: Svelte 5's `$state` is a compiler rune,
// not a runtime export, so a `bun:test` file cannot import anything from a
// module that constructs a `$state`-bearing store at load time (there is no
// preload hook for `.svelte.ts`, only for `.svelte` components — see
// `scripts/svelte-server-preload.ts`). Second, and the reason this shape is
// right even ignoring tests: keeping "what accounts exist right now" and
// "how one account's portfolio is fetched" as parameters, rather than
// closing over a store, makes it impossible to accidentally read a list
// this function captured on entry instead of one read fresh — the mistake
// this project has paid for more than once (see `AGENTS.md`, "Derive from
// what you hold, not from what you opened with").

import type { TradeAccountDto } from '$lib/api/types';
import type { Portfolio } from './view';

export interface PortfolioRefreshDeps {
	/** The accounts to refresh, read fresh on every call — never a list
	 * captured earlier. An account attached between two calls must be
	 * fetched by the second; an account removed must not still be asked
	 * about. */
	listAccounts: () => TradeAccountDto[];
	/** The account ids currently holding a portfolio, so one no longer in
	 * `listAccounts()` can be dropped rather than left stale forever. */
	listPortfolioIds: () => string[];
	/** Fetches one account's balances/positions/orders/fills. Implementations
	 * are expected to catch their own failure and return a `Portfolio` whose
	 * `error` is set (the shape `fetchOnePortfolio` in `trade.svelte.ts`
	 * follows) — but a rejection here is tolerated too, converted the same
	 * way, so one account's transport failure can never propagate out and
	 * abort the accounts still in flight. */
	fetchPortfolio: (account: TradeAccountDto) => Promise<Portfolio>;
	/** Writes one account's portfolio. Called independently per account,
	 * directly against the live store — never by building a whole new map
	 * and replacing it at the end. That is what makes an account attached
	 * mid-refresh survive (it is simply never touched, not overwritten by a
	 * snapshot that predates it) and what stops one slow adapter from
	 * blanking every account that already resolved. */
	write: (accountId: string, portfolio: Portfolio) => void;
	/** Drops a portfolio whose account is no longer in `listAccounts()`. */
	remove: (accountId: string) => void;
	/** Turns a caught error into the string `Portfolio.error` carries. */
	describeError: (error: unknown) => string;
}

/** An empty portfolio carrying only an error — what a failed or rejected
 * fetch resolves to, so the caller's card shows the failure rather than a
 * silently missing account. */
function errorPortfolio(message: string): Portfolio {
	return { balances: null, positions: [], orders: [], fills: [], error: message };
}

/** Refreshes every account's portfolio from the account list as it stands
 * right now. See `PortfolioRefreshDeps` for the guarantees each account's
 * fetch and write must hold. */
export async function refreshPortfolios(deps: PortfolioRefreshDeps): Promise<void> {
	const accounts = deps.listAccounts();
	const liveIds = new Set(accounts.map((account) => account.id));
	for (const existingId of deps.listPortfolioIds()) {
		if (!liveIds.has(existingId)) deps.remove(existingId);
	}

	await Promise.all(
		accounts.map(async (account) => {
			let portfolio: Portfolio;
			try {
				portfolio = await deps.fetchPortfolio(account);
			} catch (error) {
				// Belt and braces: `fetchPortfolio` is documented to catch its
				// own failures, but a rejection here must still not take any
				// other account's write down with it.
				portfolio = errorPortfolio(deps.describeError(error));
			}
			deps.write(account.id, portfolio);
		})
	);
}
