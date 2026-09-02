// Behaviour tests for `state/trade.svelte.ts`'s portfolio refresh, run
// against `lib/trade/portfolio-refresh.ts` — the plain module that file
// wires its runes into (see that module's own doc comment for why: a
// `bun:test` file cannot import anything from a module that constructs
// `$state` at load time, since `$state` is a compiler rune with no runtime
// meaning outside Svelte's own compile step, and this repo's `bun test`
// preload only compiles `.svelte` components, never `.svelte.ts` modules —
// `scripts/svelte-server-preload.ts`). Every deps function below is a
// stand-in for what `trade.svelte.ts` actually wires to `tradeStore`.
import { describe, expect, test } from 'bun:test';
import { refreshPortfolios, type PortfolioRefreshDeps } from '$lib/trade/portfolio-refresh';
import { emptyPortfolio, type Portfolio } from '$lib/trade/view';
import type { TradeAccountDto } from '$lib/api/types';

function account(id: string): TradeAccountDto {
	return {
		id,
		adapter_id: 'sim',
		label: id,
		owned: true,
		enabled: true,
		owner_id: 'u1',
		created_at: 0,
		updated_at: 0
	};
}

/** A promise plus its own resolver, pulled apart. Not `let x = null` plus a
 * later reassignment inside the executor: TypeScript's control-flow
 * narrowing does not see across that closure boundary and keeps reading the
 * variable as `null` at every later call site, even though the executor has
 * already run synchronously by then. A definite-assignment declaration with
 * no `null` in its flow sidesteps it. */
function deferred<T>(): { promise: Promise<T>; resolve: (value: T) => void } {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((res) => {
		resolve = res;
	});
	return { promise, resolve };
}

function portfolioWithEquity(value: string): Portfolio {
	return {
		...emptyPortfolio(),
		balances: {
			assets: [],
			currency: 'USD',
			balance: { scale: 2, value },
			equity: { scale: 2, value },
			unrealized_pnl: { scale: 2, value: '0' },
			realized_pnl: { scale: 2, value: '0' },
			margin_used: { scale: 2, value: '0' }
		}
	};
}

/** An in-memory stand-in for `tradeStore.accounts`/`tradeStore.portfolios`,
 * with the same "mutable, read fresh" shape the real store has: `accounts`
 * is read through a getter that always returns whatever the harness holds
 * right now, exactly the way a `$state` array reads in a component. */
function harness() {
	let accounts: TradeAccountDto[] = [];
	const portfolios = new Map<string, Portfolio>();
	const fetchCalls: string[] = [];
	const fetchImpls = new Map<string, () => Promise<Portfolio>>();

	const deps: PortfolioRefreshDeps = {
		listAccounts: () => accounts,
		listPortfolioIds: () => Array.from(portfolios.keys()),
		fetchPortfolio: async (acct) => {
			fetchCalls.push(acct.id);
			const impl = fetchImpls.get(acct.id);
			if (impl) return impl();
			return portfolioWithEquity('100');
		},
		write: (id, portfolio) => portfolios.set(id, portfolio),
		remove: (id) => portfolios.delete(id),
		describeError: (error) => (error instanceof Error ? error.message : String(error))
	};

	return {
		deps,
		setAccounts: (next: TradeAccountDto[]) => (accounts = next),
		setFetchImpl: (id: string, impl: () => Promise<Portfolio>) => fetchImpls.set(id, impl),
		portfolios,
		fetchCalls
	};
}

describe('refreshPortfolios', () => {
	// The regression guard this project has paid for most: a screen (or, as
	// here, the shared store) that reads the account list once and reuses
	// it silently drops every account attached since. Proven by breaking it
	// below — see this file's own comment at the bottom.
	test('reads the account list as it is when the call runs, not one captured earlier', async () => {
		const h = harness();
		h.setAccounts([account('a1')]);

		await refreshPortfolios(h.deps);
		expect(h.fetchCalls).toEqual(['a1']);
		expect(Array.from(h.portfolios.keys()).sort()).toEqual(['a1']);

		// An account attached between the two calls — the list `deps` sees
		// now has grown, and nothing in `refreshPortfolios` was told about
		// it directly; it must notice on its own by reading again.
		h.setAccounts([account('a1'), account('a2')]);

		await refreshPortfolios(h.deps);
		expect(h.fetchCalls).toEqual(['a1', 'a1', 'a2']);
		expect(Array.from(h.portfolios.keys()).sort()).toEqual(['a1', 'a2']);
	});

	test('an account removed between refreshes has its portfolio dropped', async () => {
		const h = harness();
		h.setAccounts([account('a1'), account('a2')]);
		await refreshPortfolios(h.deps);
		expect(Array.from(h.portfolios.keys()).sort()).toEqual(['a1', 'a2']);

		h.setAccounts([account('a1')]);
		await refreshPortfolios(h.deps);
		expect(Array.from(h.portfolios.keys())).toEqual(['a1']);
	});

	test("one account's failure leaves the others' portfolios intact", async () => {
		const h = harness();
		h.setAccounts([account('good'), account('bad')]);
		h.setFetchImpl('bad', async () => {
			throw new Error('adapter unreachable');
		});

		await refreshPortfolios(h.deps);

		expect(h.portfolios.get('good')?.error).toBeNull();
		expect(h.portfolios.get('good')?.balances?.equity.value).toBe('100');
		expect(h.portfolios.get('bad')?.error).toBe('adapter unreachable');
	});

	test('a slow account does not block a fast account from being written first', async () => {
		const h = harness();
		h.setAccounts([account('slow'), account('fast')]);
		const slow = deferred<Portfolio>();
		h.setFetchImpl('slow', () => slow.promise);

		const pending = refreshPortfolios(h.deps);
		// `fast` can only have resolved and written by now if it was not
		// forced to wait behind `slow` in a single-file await chain.
		await Promise.resolve();
		await Promise.resolve();
		expect(h.portfolios.has('fast')).toBe(true);
		expect(h.portfolios.has('slow')).toBe(false);

		slow.resolve(portfolioWithEquity('1'));
		await pending;
		expect(h.portfolios.has('slow')).toBe(true);
	});
});

// --- Proof this test can fail --------------------------------------------
//
// A version of `refreshPortfolios` that captures the account list once,
// via a snapshot taken at the *first* call, mimics the exact bug class
// AGENTS.md warns about (a chart re-requesting its opening 300-bar window
// instead of the current one). Running the guard test above against it is
// how that regression was confirmed to actually fail before this suite was
// trusted — see this plan's verification transcript, not re-run here as an
// automated test since it would require duplicating the buggy
// implementation permanently just to assert it stays broken.
