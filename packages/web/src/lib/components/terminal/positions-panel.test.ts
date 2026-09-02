// Renders the real, compiled widget through Svelte's own server renderer.
// The property under test: real positions across owned accounts render, the
// empty state reads as copy rather than a blank grid, and the widget never
// renders an action button — `dashboardPositions` marks every row
// non-tradable on purpose (see that function's own docs), and a button that
// silently does nothing is worse than no button.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import type { PositionDto, TradeAccountDto } from '$lib/api/types';
import { dashboardPositions, emptyPortfolio, type Portfolio } from '$lib/trade/view';
import PositionsPanel from './positions-panel.svelte';

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

function position(overrides: Partial<PositionDto> = {}): PositionDto {
	return {
		id: 'okx-spot:BTCUSDT',
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

describe('positions-panel (real DOM output)', () => {
	test('a real open position across owned accounts renders its instrument and account label', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), positions: [position()] }
		};
		const table = dashboardPositions([account()], portfolios, 'UTC');
		const { body } = render(PositionsPanel, { props: { table } });
		expect(body).toContain('okx-spot:BTCUSDT');
		expect(body).toContain('Sim Main');
	});

	test('no open positions renders the empty-state copy, not an empty grid', () => {
		const table = dashboardPositions([account()], { 'acct-1': emptyPortfolio() }, 'UTC');
		const { body } = render(PositionsPanel, { props: { table } });
		expect(body).toContain('NO OPEN POSITIONS');
	});

	test('a summary row never carries an action button, even for an owned position', () => {
		const portfolios: Record<string, Portfolio> = {
			'acct-1': { ...emptyPortfolio(), positions: [position()] }
		};
		const table = dashboardPositions([account()], portfolios, 'UTC');
		const { body } = render(PositionsPanel, { props: { table } });
		expect(body).not.toContain('CLOSE');
	});
});
