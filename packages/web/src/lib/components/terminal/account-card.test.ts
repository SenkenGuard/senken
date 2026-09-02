// Renders the real, compiled card through Svelte's own server renderer —
// the same reason `equity-card.test.ts` and `risk-panel.test.ts` do it this
// way. The properties under test: the currency travels with the equity
// figure, a degraded/disconnected account says why in visible text (not
// only a dot colour), an account whose adapter is no longer installed says
// so instead of rendering a blank or raw-id adapter name, and a portfolio
// fetch failure shows on the card itself.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import CpuIcon from '@lucide/svelte/icons/cpu';
import type { AccountCardData } from '$lib/trade/view';
import AccountCard from './account-card.svelte';

function card(overrides: Partial<AccountCardData> = {}): AccountCardData {
	return {
		id: 'acct-1',
		label: 'Sim Main',
		mode: 'DEMO',
		icon: CpuIcon,
		adapterName: 'SIMULATOR',
		adapterInstalled: true,
		currency: 'USD',
		equity: '50,000.00',
		pnl: '+250.00',
		pnlTone: 'gain',
		selected: false,
		readOnly: false,
		health: 'connected',
		healthReason: null,
		portfolioError: null,
		...overrides
	};
}

describe('account-card (real DOM output)', () => {
	test('the equity figure carries its currency', () => {
		const { body } = render(AccountCard, { props: { card: card(), onclick: () => {} } });
		expect(body).toContain('50,000.00 USD');
	});

	test('a connected account carries no reason text', () => {
		const { body } = render(AccountCard, {
			props: { card: card({ health: 'connected' }), onclick: () => {} }
		});
		expect(body).toContain('data-account-health="connected"');
		expect(body).not.toContain('data-health-reason');
	});

	test('a degraded account shows the adapter\'s own reason as visible text', () => {
		const { body } = render(AccountCard, {
			props: {
				card: card({ health: 'degraded', healthReason: 'Rate limited by the venue.' }),
				onclick: () => {}
			}
		});
		expect(body).toContain('data-account-health="degraded"');
		expect(body).toContain('data-health-reason');
		expect(body).toContain('Rate limited by the venue.');
	});

	test('a disconnected account shows its reason too', () => {
		const { body } = render(AccountCard, {
			props: {
				card: card({ health: 'disconnected', healthReason: 'Credentials rejected.' }),
				onclick: () => {}
			}
		});
		expect(body).toContain('data-account-health="disconnected"');
		expect(body).toContain('Credentials rejected.');
	});

	test('an account whose adapter is no longer installed says so, not a blank or raw id', () => {
		const { body } = render(AccountCard, {
			props: { card: card({ adapterInstalled: false, adapterName: 'ADAPTER NOT INSTALLED' }), onclick: () => {} }
		});
		expect(body).toContain('data-adapter-missing');
		expect(body).toContain('ADAPTER NOT INSTALLED');
	});

	test('a portfolio fetch failure shows on the card itself', () => {
		const { body } = render(AccountCard, {
			props: { card: card({ portfolioError: 'Connection timed out.' }), onclick: () => {} }
		});
		expect(body).toContain('data-portfolio-error');
		expect(body).toContain('Connection timed out.');
	});

	test('no failure means no error text at all', () => {
		const { body } = render(AccountCard, { props: { card: card(), onclick: () => {} } });
		expect(body).not.toContain('data-portfolio-error');
	});
});
