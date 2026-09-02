// Renders the real, compiled widget through Svelte's own server renderer —
// the same reason `execution-grid.test.ts` and `schema-form.test.ts` do it
// this way. The property under test: the widget shows what `dashboardEquity`
// actually computed (no fabricated curve, no fabricated number), and its
// empty and "no margin" states render as copy rather than a blank table.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import type { BalancesDto, TradeAccountDto } from '$lib/api/types';
import { dashboardEquity, emptyPortfolio, type Portfolio } from '$lib/trade/view';
import EquityCard from './equity-card.svelte';

function account(overrides: Partial<TradeAccountDto> = {}): TradeAccountDto {
	return {
		id: 'acct-1',
		owner_id: 'user-1',
		adapter_id: 'simulator',
		label: 'Sim Main',
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
		equity: { scale: 2, value: '10250000' },
		unrealized_pnl: { scale: 2, value: '250000' },
		realized_pnl: { scale: 2, value: '0' },
		margin_used: { scale: 2, value: '150000' },
		margin_available: { scale: 2, value: '9850000' },
		...overrides
	};
}

describe('equity-card (real DOM output)', () => {
	test('shows totalled equity and per-account PnL from real balances', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), balances: balances() }
		};
		const data = dashboardEquity([account()], portfolios);
		const { body } = render(EquityCard, { props: { data } });
		expect(body).toContain('102,500.00');
		expect(body).toContain('+2,500.00');
		expect(body).toContain('Sim Main');
		expect(body).toContain('1,500.00 / 98,500.00');
	});

	test('an account whose adapter reports no margin says so instead of a zero', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': {
				...emptyPortfolio(),
				balances: balances({ margin_used: null, margin_available: null })
			}
		};
		const data = dashboardEquity([account()], portfolios);
		const { body } = render(EquityCard, { props: { data } });
		expect(body).toContain('NO MARGIN');
		expect(body).not.toContain('1,500.00 / 98,500.00');
	});

	test('no accounts at all renders the empty state, not an empty table', () => {
		const data = dashboardEquity([], {});
		const { body } = render(EquityCard, { props: { data } });
		expect(body).toContain('NO ACCOUNTS ATTACHED');
	});

	test('no equity curve is drawn — the widget never emits an svg or canvas element', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), balances: balances() }
		};
		const data = dashboardEquity([account()], portfolios);
		const { body } = render(EquityCard, { props: { data } });
		expect(body).not.toContain('<svg');
		expect(body).not.toContain('<canvas');
	});

	test('accounts in different currencies show a count and per-currency subtotals, never a blended sum', () => {
		const accounts = [account({ id: 'a', label: 'USD Account' }), account({ id: 'b', label: 'USDT Account' })];
		const portfolios: Record<string, Portfolio> = {
			a: { ...emptyPortfolio(), balances: balances({ currency: 'USD' }) },
			b: { ...emptyPortfolio(), balances: balances({ currency: 'USDT', equity: { scale: 2, value: '500000' } }) }
		};
		const data = dashboardEquity(accounts, portfolios);
		const { body } = render(EquityCard, { props: { data } });
		expect(body).toContain('2 accounts · 2 currencies');
		expect(body).toContain('data-currency-subtotals');
		expect(body).toContain('102,500.00 USD');
		expect(body).toContain('5,000.00 USDT');
	});
});
