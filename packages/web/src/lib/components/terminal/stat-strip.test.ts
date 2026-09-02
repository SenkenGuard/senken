// Renders the real, compiled strip through Svelte's own server renderer.
// The property under test: a stat's `detail` lines (the per-currency
// subtotals `aggregateStats` produces when an overview spans more than one
// currency) actually render, and a stat with no `detail` renders none.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import type { StatItem } from '$lib/trade/view';
import StatStrip from './stat-strip.svelte';

describe('stat-strip (real DOM output)', () => {
	test('a plain stat renders its label and value with no detail lines', () => {
		const stats: StatItem[] = [{ label: 'TOTAL EQUITY', value: '50,000.00 USD', tone: 'fg' }];
		const { body } = render(StatStrip, { props: { stats } });
		expect(body).toContain('50,000.00 USD');
		expect(body).not.toContain('data-stat-detail');
	});

	test('a mixed-currency stat renders every subtotal line underneath', () => {
		const stats: StatItem[] = [
			{
				label: 'TOTAL EQUITY',
				value: '2 accounts · 2 currencies',
				tone: 'fg2',
				detail: ['100.00 USD', '50.00 USDT']
			}
		];
		const { body } = render(StatStrip, { props: { stats } });
		expect(body).toContain('2 accounts · 2 currencies');
		expect(body).toContain('data-stat-detail');
		expect(body).toContain('100.00 USD');
		expect(body).toContain('50.00 USDT');
	});
});
