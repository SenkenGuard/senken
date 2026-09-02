// The property this plan exists to guarantee on the web side: row-level
// actions (CLOSE, CANCEL, AMEND) only ever appear when the account behind
// the row can actually be traded — never on a read-only account (001's
// rule, visible), and never on an order that can no longer change.
import { describe, expect, test } from 'bun:test';
import type {
	AccountAccessDto,
	AdapterCapabilitiesDto,
	BalancesDto,
	OrderDto,
	PositionDto,
	ScaledDto,
	TradeAccountDto
} from '$lib/api/types';
import {
	aggregateStats,
	anyPortfolioLoading,
	dashboardEquity,
	dashboardPositions,
	dashboardRisk,
	emptyPortfolio,
	engineTable,
	exposureByAdapter,
	formatTime,
	hudStats,
	isAccountTradable,
	mulScaled,
	sumByCurrency,
	type Portfolio
} from './view';

const baseCapabilities: AdapterCapabilitiesDto = {
	order_kinds: ['market', 'limit'],
	time_in_force: ['gtc'],
	quantity_unit: 'base',
	position_mode: 'spot_holdings',
	features: []
};

function order(overrides: Partial<OrderDto> = {}): OrderDto {
	return {
		id: 'o1',
		client_order_id: null,
		account_id: 'acct-1',
		instrument: 'okx-spot:BTCUSDT',
		side: 'buy',
		kind: 'limit',
		limit_price: { scale: 2, value: '10000' },
		trigger_price: null,
		quantity: { scale: 3, value: '250' },
		filled_quantity: { scale: 3, value: '0' },
		average_price: null,
		time_in_force: 'gtc',
		status: 'open',
		reduce_only: false,
		submitted_at: 0,
		updated_at: 0,
		reject_reason: null,
		...overrides
	};
}

function position(overrides: Partial<PositionDto> = {}): PositionDto {
	return {
		account_id: 'acct-1',
		instrument: 'okx-spot:BTCUSDT',
		side: 'long',
		quantity: { scale: 3, value: '250' },
		average_entry: { scale: 2, value: '6800000' },
		mark_price: null,
		unrealized_pnl: null,
		realized_pnl: { scale: 2, value: '0' },
		margin: null,
		leverage: null,
		opened_at: 0,
		...overrides
	};
}

function account(overrides: Partial<TradeAccountDto> = {}): TradeAccountDto {
	return {
		id: 'acct-1',
		owner_id: 'user-1',
		adapter_id: 'simulator',
		label: 'Main',
		enabled: true,
		owned: true,
		created_at: 0,
		updated_at: 0,
		...overrides
	};
}

function balances(overrides: Partial<BalancesDto> = {}): BalancesDto {
	return {
		assets: [],
		currency: 'USD',
		balance: { scale: 2, value: '10000000' },
		equity: { scale: 2, value: '10000000' },
		unrealized_pnl: { scale: 2, value: '0' },
		margin_used: { scale: 2, value: '0' },
		margin_available: { scale: 2, value: '10000000' },
		...overrides
	};
}

describe('engineTable order actions', () => {
	test('a working order on a tradable account carries CANCEL and AMEND', () => {
		const table = engineTable(
			'orders',
			'account',
			[],
			[{ ...order(), accountId: 'acct-1', accountLabel: 'Main', tradable: true }],
			[],
			'UTC'
		);
		expect(table.rows[0].actions?.map((a) => a.key)).toEqual(['cancel', 'amend']);
	});

	test('a filled order carries no actions even on a tradable account', () => {
		const table = engineTable(
			'orders',
			'account',
			[],
			[
				{
					...order({ status: 'filled' }),
					accountId: 'acct-1',
					accountLabel: 'Main',
					tradable: true
				}
			],
			[],
			'UTC'
		);
		expect(table.rows[0].actions).toBeUndefined();
	});

	test('a working order on a non-tradable account carries no actions at all', () => {
		const table = engineTable(
			'orders',
			'account',
			[],
			[{ ...order(), accountId: 'acct-1', accountLabel: 'Main', tradable: false }],
			[],
			'UTC'
		);
		expect(table.rows[0].actions).toBeUndefined();
	});
});

describe('engineTable position actions', () => {
	test('an open position on a tradable account carries CLOSE', () => {
		const table = engineTable(
			'positions',
			'account',
			[{ ...position(), accountId: 'acct-1', accountLabel: 'Main', tradable: true }],
			[],
			[],
			'UTC'
		);
		expect(table.rows[0].actions?.map((a) => a.key)).toEqual(['close']);
	});

	test('a position on a read-only (non-tradable) account carries no actions', () => {
		const table = engineTable(
			'positions',
			'account',
			[{ ...position(), accountId: 'acct-1', accountLabel: 'Main', tradable: false }],
			[],
			[],
			'UTC'
		);
		expect(table.rows[0].actions).toBeUndefined();
	});
});

describe('isAccountTradable', () => {
	test('an owned, enabled account with no loaded access entry reads as tradable', () => {
		// Absence of an entry means "not yet loaded", read the same as
		// unrestricted rather than blocking on a lookup that has not
		// answered yet — the same rule `state/trade.svelte.ts` documents.
		expect(isAccountTradable(account(), {})).toBe(true);
	});

	test('a read-only account is not tradable', () => {
		const access: Record<string, AccountAccessDto> = {
			'acct-1': { level: 'read_only', capabilities: baseCapabilities, note: 'read-only' }
		};
		expect(isAccountTradable(account(), access)).toBe(false);
	});

	test('an unowned account is not tradable, whatever its access says', () => {
		expect(isAccountTradable(account({ owned: false }), {})).toBe(false);
	});

	test('a disabled account is not tradable', () => {
		expect(isAccountTradable(account({ enabled: false }), {})).toBe(false);
	});
});

describe('mulScaled', () => {
	test('multiplies two decimals exactly, scales added rather than widened', () => {
		// 0.25 (scale 2) * 69000.00 (scale 2) = 17250.0000 at scale 4.
		const qty: ScaledDto = { scale: 2, value: '25' };
		const price: ScaledDto = { scale: 2, value: '6900000' };
		expect(mulScaled(qty, price)).toEqual({ scale: 4, value: '172500000' });
	});

	test('stays exact past where a double would already have rounded', () => {
		// 2^53 + 1 cannot be represented exactly as a JS number — Number()
		// on it silently rounds down to 2^53 before any multiplication even
		// happens, which is exactly the failure mode this function exists to
		// avoid for a value that is still money.
		const unsafe = '9007199254740993';
		expect(Number.isSafeInteger(Number(unsafe))).toBe(false);
		const a: ScaledDto = { scale: 0, value: unsafe };
		const b: ScaledDto = { scale: 0, value: '3' };
		expect(mulScaled(a, b)).toEqual({ scale: 0, value: '27021597764222979' });
	});
});

describe('dashboardEquity', () => {
	test('totals equity, balance and PnL across accounts with addScaled, not Number(), and labels the currency', () => {
		// The second account's equity is one digit past 2^53's safe integer
		// range; summing through `Number()` would silently lose it. Both
		// accounts share USD (the `balances()` fixture default), so this is
		// the single-currency case: a real sum, with its unit attached.
		const accounts = [account({ id: 'a', label: 'A' }), account({ id: 'b', label: 'B' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ equity: { scale: 2, value: '9007199254740993' } }) },
			b: { ...emptyPortfolio(), balances: balances({ equity: { scale: 2, value: '7' } }) }
		};
		const data = dashboardEquity(accounts, portfolios);
		expect(data.totals?.mixedCurrency).toBe(false);
		expect(data.totals?.equity).toBe('90,071,992,547,410.00 USD');
	});

	test('accounts in different currencies are never blended into one total', () => {
		const accounts = [account({ id: 'a', label: 'A' }), account({ id: 'b', label: 'B' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ currency: 'USD', equity: { scale: 2, value: '5000000' } }) },
			b: { ...emptyPortfolio(), balances: balances({ currency: 'USDT', equity: { scale: 2, value: '1234500' } }) }
		};
		const data = dashboardEquity(accounts, portfolios);
		expect(data.totals?.mixedCurrency).toBe(true);
		expect(data.totals?.equity).toBe('2 accounts · 2 currencies');
		const usd = data.totals?.perCurrency.find((s) => s.currency === 'USD');
		const usdt = data.totals?.perCurrency.find((s) => s.currency === 'USDT');
		expect(usd?.equity).toBe('50,000.00');
		expect(usdt?.equity).toBe('12,345.00');
		// Never the sum of the two currencies' digits under either unit.
		expect(data.totals?.equity).not.toContain('62,345.00');
	});

	test('an account whose balances have not loaded yet shows dashes, not zeroes', () => {
		const data = dashboardEquity([account()], {});
		expect(data.rows[0].equity).toBe('—');
		expect(data.rows[0].hasMargin).toBe(false);
	});

	test('a spot account with margin_used: null is "no margin", not a loading state', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': {
				...emptyPortfolio(),
				balances: balances({ margin_used: null, margin_available: null })
			}
		};
		const data = dashboardEquity([account()], portfolios);
		expect(data.rows[0].hasMargin).toBe(false);
		expect(data.rows[0].marginUsed).toBeNull();
	});

	test('no accounts at all totals to null rather than a zero total', () => {
		expect(dashboardEquity([], {}).totals).toBeNull();
	});
});

describe('sumByCurrency', () => {
	test('sums each currency separately rather than blending them', () => {
		const accounts = [account({ id: 'a', label: 'A' }), account({ id: 'b', label: 'B' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ currency: 'USD', equity: { scale: 2, value: '10000' } }) },
			b: { ...emptyPortfolio(), balances: balances({ currency: 'USDT', equity: { scale: 2, value: '20000' } }) }
		};
		const totals = sumByCurrency(accounts, portfolios);
		expect(totals).toHaveLength(2);
		expect(totals.find((t) => t.currency === 'USD')?.equity).toEqual({ scale: 2, value: '10000' });
		expect(totals.find((t) => t.currency === 'USDT')?.equity).toEqual({ scale: 2, value: '20000' });
	});

	test('two accounts sharing a currency are summed together, exactly', () => {
		const accounts = [account({ id: 'a', label: 'A' }), account({ id: 'b', label: 'B' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ currency: 'USD', equity: { scale: 2, value: '10000' } }) },
			b: { ...emptyPortfolio(), balances: balances({ currency: 'USD', equity: { scale: 2, value: '5000' } }) }
		};
		const totals = sumByCurrency(accounts, portfolios);
		expect(totals).toHaveLength(1);
		expect(totals[0].currency).toBe('USD');
		expect(totals[0].equity).toEqual({ scale: 2, value: '15000' });
	});

	test('an account whose balances have not loaded contributes to no currency', () => {
		const totals = sumByCurrency([account()], {});
		expect(totals).toHaveLength(0);
	});
});

describe('sumByCurrency margin', () => {
	test('margins are summed when every account in the currency reports one', () => {
		const accounts = [account({ id: 'a' }), account({ id: 'b' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ margin_used: { scale: 2, value: '1000' } }) },
			b: { ...emptyPortfolio(), balances: balances({ margin_used: { scale: 2, value: '2500' } }) }
		};
		expect(sumByCurrency(accounts, portfolios)[0].marginUsed).toEqual({ scale: 2, value: '3500' });
	});

	test('one account that reports no margin makes the currency total unknown, not understated', () => {
		const accounts = [account({ id: 'a' }), account({ id: 'b' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ margin_used: { scale: 2, value: '1000' } }) },
			b: { ...emptyPortfolio(), balances: balances({ margin_used: undefined }) }
		};
		const total = sumByCurrency(accounts, portfolios)[0];
		expect(total.marginUsed).toBeNull();
		// The equity beside it is still a real sum — one missing optional
		// field must not blank the figures that were reported.
		expect(total.equity).toEqual({ scale: 2, value: '20000000' });
	});
});

describe('hudStats', () => {
	test('equity is the accounts\' own total, with its currency', () => {
		const accounts = [account({ id: 'a' }), account({ id: 'b' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ equity: { scale: 2, value: '10000' } }) },
			b: { ...emptyPortfolio(), balances: balances({ equity: { scale: 2, value: '5000' } }) }
		};
		expect(hudStats(accounts, portfolios).find((s) => s.key === 'equity')?.value).toBe('150.00 USD');
	});

	test('with nothing attached every money cell is a dash, never a figure', () => {
		const stats = hudStats([], {});
		for (const key of ['equity', 'pnl', 'balance', 'margin', 'available']) {
			expect(stats.find((s) => s.key === key)?.value).toBe('—');
		}
		expect(stats.find((s) => s.key === 'positions')?.value).toBe('0');
	});

	test('counts positions and working orders across every account', () => {
		const accounts = [account({ id: 'a' }), account({ id: 'b' })];
		const portfolios: Record<string, Portfolio> = {
			a: {
				...emptyPortfolio(),
				positions: [position()],
				orders: [order({ status: 'open' }), order({ id: 'o2', status: 'filled' })]
			},
			b: { ...emptyPortfolio(), positions: [position()], orders: [order({ id: 'o3', status: 'pending' })] }
		};
		const stats = hudStats(accounts, portfolios);
		expect(stats.find((s) => s.key === 'positions')?.value).toBe('2');
		expect(stats.find((s) => s.key === 'working')?.value).toBe('2');
	});

	test('mixed currencies are never blended into one headline figure', () => {
		const accounts = [account({ id: 'a' }), account({ id: 'b' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ currency: 'USD' }) },
			b: { ...emptyPortfolio(), balances: balances({ currency: 'USDT' }) }
		};
		expect(hudStats(accounts, portfolios).find((s) => s.key === 'equity')?.value).toBe(
			'2 accounts · 2 currencies'
		);
	});
});

describe('aggregateStats', () => {
	test('a single shared currency produces one labelled total, not a currency count', () => {
		const accounts = [account({ id: 'a', label: 'A' }), account({ id: 'b', label: 'B' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ equity: { scale: 2, value: '10000' } }) },
			b: { ...emptyPortfolio(), balances: balances({ equity: { scale: 2, value: '5000' } }) }
		};
		const stats = aggregateStats(accounts, portfolios);
		const equity = stats.find((s) => s.label === 'TOTAL EQUITY');
		expect(equity?.value).toBe('150.00 USD');
		expect(equity?.detail).toBeUndefined();
	});

	test('accounts in different currencies show a count instead of a blended sum, with subtotals', () => {
		const accounts = [account({ id: 'a', label: 'A' }), account({ id: 'b', label: 'B' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ currency: 'USD', equity: { scale: 2, value: '10000' } }) },
			b: { ...emptyPortfolio(), balances: balances({ currency: 'USDT', equity: { scale: 2, value: '5000' } }) }
		};
		const stats = aggregateStats(accounts, portfolios);
		const equity = stats.find((s) => s.label === 'TOTAL EQUITY');
		expect(equity?.value).toBe('2 accounts · 2 currencies');
		expect(equity?.detail).toContain('100.00 USD');
		expect(equity?.detail).toContain('50.00 USDT');
	});

	test('no accounts at all reads as a dash, not "0 accounts · 0 currencies"', () => {
		const stats = aggregateStats([], {});
		expect(stats.find((s) => s.label === 'TOTAL EQUITY')?.value).toBe('—');
	});
});

describe('anyPortfolioLoading', () => {
	test('true when an account in scope has no portfolio entry at all', () => {
		expect(anyPortfolioLoading([account()], {})).toBe(true);
	});

	test('false once every account in scope has an entry, even an empty one', () => {
		expect(anyPortfolioLoading([account()], { 'acct-1': emptyPortfolio() })).toBe(false);
	});
});

describe('dashboardPositions', () => {
	test('every row is marked non-tradable, whatever the account access says', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), positions: [position()] }
		};
		const table = dashboardPositions([account()], portfolios, 'UTC');
		expect(table.rows[0].actions).toBeUndefined();
	});

	test('carries the account label onto each row', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), positions: [position()] }
		};
		const table = dashboardPositions([account({ label: 'Live BTC' })], portfolios, 'UTC');
		expect(table.rows[0].cells.some((c) => c.value === 'Live BTC')).toBe(true);
	});
});

describe('dashboardRisk', () => {
	test('an account with margin_used: null is excluded from the margin gauges as "no margin"', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': {
				...emptyPortfolio(),
				balances: balances({ margin_used: null, margin_available: null })
			}
		};
		const data = dashboardRisk([account()], portfolios);
		expect(data.margin).toHaveLength(1);
		expect(data.margin[0].hasMargin).toBe(false);
		expect(data.margin[0].usedPct).toBe(0);
	});

	test('an account whose balances have not loaded yet contributes no margin row at all', () => {
		const data = dashboardRisk([account()], {});
		expect(data.margin).toHaveLength(0);
	});

	test('notional is computed exactly via mulScaled, not Number(quantity) * Number(price)', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': {
				...emptyPortfolio(),
				balances: balances({ equity: { scale: 2, value: '1000000000' } }), // 10,000,000.00
				positions: [
					position({
						quantity: { scale: 3, value: '250' }, // 0.250
						mark_price: { scale: 2, value: '6900000' } // 69,000.00
					})
				]
			}
		};
		const data = dashboardRisk([account()], portfolios);
		expect(data.exposure).toHaveLength(1);
		expect(data.exposure[0].notional).toBe('17,250.00');
	});

	test('a position with no mark price is left out of the exposure bars', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), positions: [position({ mark_price: null })] }
		};
		const data = dashboardRisk([account()], portfolios);
		expect(data.exposure).toHaveLength(0);
	});
});

// The currency sweep: `aggregateStats` and `dashboardEquity` were fixed to
// stop blending currencies, and these two were the rest of the same shape.
// A share of a total that spans two currencies is a share of a number that
// does not exist — there is no rate anywhere in this system.
describe('exposureByAdapter across currencies', () => {
	const adapters = [
		{
			id: 'simulator',
			name: 'Senken Simulator',
			kind: 'simulation',
			description: '',
			trades_real_money: false,
			capabilities: baseCapabilities,
			coverage: { coverage: 'universal' as const },
			settings_schema: { fields: [] },
			actions: []
		}
	];

	test('one currency behaves as before, and carries its unit', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), balances: balances() }
		};
		const items = exposureByAdapter([account()], adapters, portfolios);
		expect(items).toHaveLength(1);
		expect(items[0].pct).toBe(100);
		expect(items[0].currency).toBe('USD');
	});

	test('two currencies produce a row each, and each share is of its own currency', () => {
		// Without the fix these collapse into one bar whose percentage is a
		// share of USD-plus-USDT, a quantity nothing in this system can
		// produce.
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), balances: balances({ currency: 'USD' }) },
			'acct-2': { ...emptyPortfolio(), balances: balances({ currency: 'USDT' }) }
		};
		const items = exposureByAdapter(
			[account(), account({ id: 'acct-2', label: 'Second' })],
			adapters,
			portfolios
		);

		expect(items).toHaveLength(2);
		expect(items.map((item) => item.currency).sort()).toEqual(['USD', 'USDT']);
		for (const item of items) {
			expect(item.pct).toBe(100);
		}
	});

	test('an account whose balances have not loaded contributes no row', () => {
		expect(exposureByAdapter([account()], adapters, {})).toHaveLength(0);
	});
});

describe('dashboardRisk across currencies', () => {
	const twoCurrencies: Record<string, Portfolio> = {
		'acct-1': {
			...emptyPortfolio(),
			balances: balances({ currency: 'USD' }),
			positions: [
				position({
					quantity: { scale: 3, value: '250' },
					mark_price: { scale: 2, value: '6900000' }
				})
			]
		},
		'acct-2': {
			...emptyPortfolio(),
			balances: balances({ currency: 'USDT' })
		}
	};

	test('one currency still gives a real exposure percentage', () => {
		// 0.250 at 69,000.00 is a notional of 17,250.00 against an equity of
		// 34,500.00 — exactly half, so the assertion says something.
		const portfolios: Record<string, Portfolio> = {
			'acct-1': {
				...emptyPortfolio(),
				balances: balances({ equity: { scale: 2, value: '3450000' } }),
				positions: [
					position({
						quantity: { scale: 3, value: '250' },
						mark_price: { scale: 2, value: '6900000' }
					})
				]
			}
		};
		expect(dashboardRisk([account()], portfolios).exposure[0].pct).toBe(50);
	});

	test('more than one currency withholds the percentage but keeps the amount', () => {
		const data = dashboardRisk(
			[account(), account({ id: 'acct-2', label: 'Second' })],
			twoCurrencies
		);

		expect(data.exposure).toHaveLength(1);
		expect(data.exposure[0].notional).toBe('17,250.00');
		expect(data.exposure[0].pct).toBe(null);
	});
});

describe('formatTime — the reader\'s own display zone, not the browser\'s', () => {
	test('renders the same instant differently depending on the zone given', () => {
		const nanos = 3600 * 1_000_000_000; // 1970-01-01T01:00:00Z
		expect(formatTime(nanos, 'UTC')).toBe('01:00:00');
		expect(formatTime(nanos, 'America/New_York')).toBe('20:00:00');
	});
});

describe('engineTable TIME column', () => {
	test('renders an order\'s TIME cell in the zone passed in, not a fixed one', () => {
		const submittedAt = 3600 * 1_000_000_000; // 1970-01-01T01:00:00Z
		const table = engineTable(
			'orders',
			'account',
			[],
			[
				{
					...order({ submitted_at: submittedAt }),
					accountId: 'acct-1',
					accountLabel: 'Main',
					tradable: true
				}
			],
			[],
			'America/New_York'
		);
		expect(table.rows[0].cells[0].value).toBe('20:00:00');
	});
});
