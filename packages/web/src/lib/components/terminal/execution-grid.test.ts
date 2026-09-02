// Renders the real, compiled grid through Svelte's own server renderer and
// reads the actual markup — the same reason `schema-form.test.ts` and
// `order-ticket.test.ts` do it this way rather than asserting on the logic
// that is supposed to produce the markup. The property under test: the
// trailing actions column exists only when at least one row actually has
// something to put in it.
//
// This renders `execution-grid.svelte` directly rather than the whole
// `executions-table.svelte`: the latter wraps this in `Tabs.Root`
// (bits-ui), which touches browser-only state at module load and cannot be
// compiled through this project's server-render harness — see that
// component's own docs.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import type { EngineTable } from '$lib/trade/view';
import ExecutionGrid from './execution-grid.svelte';

function baseTable(): EngineTable {
	return {
		tabs: [
			{ key: 'positions', label: 'POSITIONS', count: 1 },
			{ key: 'orders', label: 'ORDERS', count: 0 },
			{ key: 'exec', label: 'EXECUTIONS', count: 0 }
		],
		columns: [
			{ label: 'INSTRUMENT', align: 'left' },
			{ label: 'SIDE', align: 'left' }
		],
		rows: [
			{
				key: 'acct-1:okx-spot:BTCUSDT',
				cells: [
					{ value: 'okx-spot:BTCUSDT', tone: 'fg', align: 'left' },
					{ value: 'LONG', tone: 'gain', align: 'left' }
				]
			}
		],
		emptyLabel: 'NO OPEN POSITIONS',
		loading: false
	};
}

describe('execution-grid (real DOM output)', () => {
	test('no row carrying actions means no trailing actions column at all', () => {
		const { body } = render(ExecutionGrid, { props: { table: baseTable() } });
		expect(body).not.toContain('CLOSE');
	});

	test('a row carrying an action renders its button', () => {
		const table = baseTable();
		table.rows = [
			{ ...table.rows[0], actions: [{ key: 'close', label: 'CLOSE', tone: 'destructive' }] }
		];
		const { body } = render(ExecutionGrid, { props: { table } });
		expect(body).toContain('CLOSE');
	});

	test('a row with more than one action renders every button', () => {
		const table = baseTable();
		table.rows = [
			{
				...table.rows[0],
				actions: [
					{ key: 'cancel', label: 'CANCEL', tone: 'destructive' },
					{ key: 'amend', label: 'AMEND', tone: 'default' }
				]
			}
		];
		const { body } = render(ExecutionGrid, { props: { table } });
		expect(body).toContain('CANCEL');
		expect(body).toContain('AMEND');
	});

	test('an empty table (no rows at all) still renders no actions column', () => {
		const table = baseTable();
		table.rows = [];
		const { body } = render(ExecutionGrid, { props: { table } });
		expect(body).toContain(table.emptyLabel);
		expect(body).not.toContain('CLOSE');
	});

	test('a table still loading shows a loading message, not the empty-state copy', () => {
		const table = baseTable();
		table.rows = [];
		table.loading = true;
		const { body } = render(ExecutionGrid, { props: { table } });
		expect(body).toContain('LOADING');
		expect(body).not.toContain(table.emptyLabel);
	});

	test('a loading table with rows already in hand shows the rows, not a loading message', () => {
		// Loading only ever describes the *empty* case — an account whose
		// portfolio has not answered yet contributes nothing, so this can
		// only be true when `rows` is also empty in practice. Proven here
		// anyway: rows already present must never be hidden behind a
		// loading message.
		const table = baseTable();
		table.loading = true;
		const { body } = render(ExecutionGrid, { props: { table } });
		expect(body).not.toContain('LOADING…');
		expect(body).toContain('okx-spot:BTCUSDT');
	});
});
