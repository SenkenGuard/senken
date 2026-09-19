// Interactive proof for the picker's search, keyboard selection and badges —
// none of which the server-only Svelte test harness can exercise, since all
// three need a real focused input and real keyboard events in a real
// browser.
import { test, expect, vi } from 'vitest';
import { page, userEvent } from 'vitest/browser';
import { render } from 'vitest-browser-svelte';
import AddWidgetPicker from './add-widget-picker.svelte';
import type { DashboardWidgetDefinition } from './api';

const CATALOG: DashboardWidgetDefinition[] = [
	{
		widget_type_id: 'senken/equity',
		provider_id: 'senken',
		title: 'Equity Curve',
		description: 'balance across every account',
		default_size: { width: 6, height: 4 },
		min_size: { width: 6, height: 4 },
		data_source: 'live'
	},
	{
		widget_type_id: 'senken/positions',
		provider_id: 'senken',
		title: 'Open Positions',
		description: 'live PnL table',
		default_size: { width: 6, height: 4 },
		min_size: { width: 6, height: 4 },
		data_source: 'live'
	},
	{
		widget_type_id: 'senken/position-size',
		provider_id: 'senken',
		title: 'Position Size Calculator',
		description: 'balance, risk percent and stop distance, computed exactly',
		default_size: { width: 4, height: 5 },
		min_size: { width: 4, height: 5 },
		data_source: 'live'
	},
	{
		// Not a real built-in — stands in for a plugin-contributed widget
		// with `data_source: 'mock'`, the one case this catalog's own four
		// built-ins never exercise (`widget-registry.ts`'s own note on why
		// every built-in now reports `live`).
		widget_type_id: 'example/clock',
		provider_id: 'example',
		title: 'Clock',
		description: 'shows the current time',
		default_size: { width: 2, height: 2 },
		min_size: { width: 2, height: 2 },
		data_source: 'mock'
	}
];

test('typing a query narrows to one row, and Enter picks it', async () => {
	const onPick = vi.fn();
	// `open` is a plain, caller-owned prop here (the real caller,
	// `routes/dashboard/+page.svelte`, flips its own `addWidgetOpen` state
	// to `false` when this fires) — mirrored with `rerender` below, since a
	// static `open: true` prop would never let the dialog actually close no
	// matter how many times `onClose` itself gets called.
	const screen = await render(AddWidgetPicker, {
		open: true,
		catalog: CATALOG,
		placedTypeIds: new Set<string>(),
		onClose: () => screen.rerender({ open: false }),
		onPick
	});

	await userEvent.keyboard('calculator');
	const options = page.getByRole('option');
	await expect.poll(() => options.elements().length).toBe(1);
	await expect.element(options.first()).toHaveAttribute('aria-selected', 'true');

	await userEvent.keyboard('{Enter}');

	expect(onPick).toHaveBeenCalledTimes(1);
	expect(onPick.mock.calls[0][0].widget_type_id).toBe('senken/position-size');
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
});

test('a row already placed in the workspace shows an Added badge', async () => {
	await render(AddWidgetPicker, {
		open: true,
		catalog: CATALOG,
		placedTypeIds: new Set(['senken/equity']),
		onClose: () => {},
		onPick: () => {}
	});

	const equityRow = page.getByRole('option', { name: /Equity Curve/ });
	await expect.element(equityRow.getByText('Added')).toBeInTheDocument();

	const positionsRow = page.getByRole('option', { name: /Open Positions/ });
	await expect.element(positionsRow.getByText('Added')).not.toBeInTheDocument();
});

test('a mock-data-source row carries a Mockup badge', async () => {
	await render(AddWidgetPicker, {
		open: true,
		catalog: CATALOG,
		placedTypeIds: new Set<string>(),
		onClose: () => {},
		onPick: () => {}
	});

	const clockRow = page.getByRole('option', { name: /Clock/ });
	await expect.element(clockRow.getByText('Mockup')).toBeInTheDocument();

	const equityRow = page.getByRole('option', { name: /Equity Curve/ });
	await expect.element(equityRow.getByText('Mockup')).not.toBeInTheDocument();
});

test('a query with no match shows an empty state with a way back', async () => {
	await render(AddWidgetPicker, {
		open: true,
		catalog: CATALOG,
		placedTypeIds: new Set<string>(),
		onClose: () => {},
		onPick: () => {}
	});

	await userEvent.keyboard('nonexistentwidgetxyz');
	await expect.element(page.getByText('No widgets match')).toBeInTheDocument();

	await userEvent.click(page.getByRole('button', { name: 'Clear search' }));
	await expect.element(page.getByRole('option').first()).toBeInTheDocument();
	await expect.poll(() => page.getByRole('option').elements().length).toBe(CATALOG.length);
});
