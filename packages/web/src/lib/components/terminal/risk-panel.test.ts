// Renders the real, compiled widget through Svelte's own server renderer.
// The property under test: an account with no margin (`margin_used: null` —
// a spot venue genuinely has none) shows the "no margin" sentence rather
// than a bar sitting at 0%, which would read as "no risk" instead of "not
// applicable" — the exact distinction the plan this widget implements
// exists to draw.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import type { BalancesDto, PositionDto, TradeAccountDto } from '$lib/api/types';
import { dashboardRisk, emptyPortfolio, type Portfolio } from '$lib/trade/view';
import RiskPanel from './risk-panel.svelte';

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
		equity: { scale: 2, value: '10000000' },
		unrealized_pnl: { scale: 2, value: '0' },
		margin_used: { scale: 2, value: '2000000' },
		margin_available: { scale: 2, value: '8000000' },
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
		mark_price: { scale: 2, value: '6900000' },
		unrealized_pnl: { scale: 2, value: '25000' },
		realized_pnl: { scale: 2, value: '0' },
		margin: null,
		leverage: null,
		opened_at: 0,
		...overrides
	};
}

describe('risk-panel (real DOM output)', () => {
	test('an account with margin renders a labelled gauge', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), balances: balances() }
		};
		const data = dashboardRisk([account()], portfolios);
		const { body } = render(RiskPanel, { props: { data } });
		expect(body).toContain('Sim Main'.toUpperCase());
		expect(body).toContain('20,000.00 / 100,000.00 USD');
	});

	test('a spot account with no margin says so instead of drawing a zeroed gauge', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': {
				...emptyPortfolio(),
				balances: balances({ margin_used: null, margin_available: null })
			}
		};
		const data = dashboardRisk([account()], portfolios);
		const { body } = render(RiskPanel, { props: { data } });
		expect(body).toContain('NO MARGIN ON THIS ACCOUNT');
		expect(body).not.toContain('0%');
	});

	test('an instrument held renders as a notional share of total equity', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), balances: balances(), positions: [position()] }
		};
		const data = dashboardRisk([account()], portfolios);
		const { body } = render(RiskPanel, { props: { data } });
		expect(body).toContain('OKX-SPOT:BTCUSDT');
	});

	test('no accounts at all renders the empty state', () => {
		const data = dashboardRisk([], {});
		const { body } = render(RiskPanel, { props: { data } });
		expect(body).toContain('NO ACCOUNTS ATTACHED');
	});
});
