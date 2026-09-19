// A real pointer drag, in a real browser, against the real component — the
// only environment in which a genuine "body locked" defect can be told apart
// from the harness artifact this project has been fooled by before: a
// hidden test pane never fires `requestAnimationFrame`, which starves a
// component's own unmount lifecycle and looks exactly like a stuck body or
// dialog when nothing is actually wrong (see `browser-setup.ts`'s probe for
// this). Two properties: dragging a widget's header actually moves it (proving the
// header-as-handle change in this same step did not silently break the
// existing drag path), and the `cursor-grabbing`/`select-none` classes this
// step adds to `document.body` while a drag is live are always gone again
// once the pointer is released — never left stuck the way this project
// once wrongly blamed for a hidden test pane's own `requestAnimationFrame`
// starvation.
import { test, expect, vi, afterEach } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { page, userEvent } from 'vitest/browser';
import { Toaster } from 'svelte-sonner';
import DashboardGrid from './dashboard-grid.svelte';
import type { DashboardLayoutDto, DashboardWidgetDefinition } from './api';

function jsonResponse(body: unknown): Response {
	return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });
}

function makeLayout(): DashboardLayoutDto {
	return {
		workspace: { id: 'ws1', owner_id: 'u1', name: 'Default', columns: 6, revision: 1, created_at: 0, updated_at: 0 },
		widgets: [
			{
				id: 'w1',
				provider_id: 'senken',
				widget_type_id: 'senken/equity',
				position_x: 0,
				position_y: 0,
				width: 3,
				height: 3,
				visible: true,
				config: '{}',
				config_schema_version: 1,
				created_at: 0,
				updated_at: 0
			},
			{
				// Stacked *below* w1, not beside it — leaving w1 the full
				// `columns - width` of horizontal room a drag test needs, so
				// this widget existing at all never itself becomes the
				// overlap that blocks the move under test.
				id: 'w2',
				provider_id: 'senken',
				widget_type_id: 'senken/risk',
				position_x: 0,
				position_y: 3,
				width: 3,
				height: 3,
				visible: true,
				config: '{}',
				config_schema_version: 1,
				created_at: 0,
				updated_at: 0
			}
		]
	};
}

const catalog: DashboardWidgetDefinition[] = [];

/** Fires a synthetic pointer drag sequence on `el`: `pointerdown` at
 * `(x, y)`, one `pointermove` to `(x + dx, y + dy)`, then `pointerup` — the
 * exact three event types `dashboard-grid.svelte`'s own handlers listen
 * for (`onpointerdown` on the widget header/resize handle,
 * `onpointermove`/`onpointerup` on the grid container itself, which a
 * bubbling synthetic event reaches the same way a real one would). */
function dragBy(el: Element, x: number, y: number, dx: number, dy: number): void {
	const pointerId = 1;
	el.dispatchEvent(
		new PointerEvent('pointerdown', { bubbles: true, clientX: x, clientY: y, pointerId, isPrimary: true })
	);
	el.dispatchEvent(
		new PointerEvent('pointermove', {
			bubbles: true,
			clientX: x + dx,
			clientY: y + dy,
			pointerId,
			isPrimary: true
		})
	);
	el.dispatchEvent(
		new PointerEvent('pointerup', { bubbles: true, clientX: x + dx, clientY: y + dy, pointerId, isPrimary: true })
	);
}

afterEach(() => {
	// Belt and braces: a failed assertion mid-drag must not leave a stray
	// cursor class on `document.body` for the next test in this file.
	document.body.classList.remove('select-none', 'cursor-grabbing', 'cursor-nwse-resize');
	// This file's own tests `vi.stubGlobal('fetch', …)`; `chart-pane.browser-test.ts`
	// and `indicator-catalog.browser-test.ts` already unstub after
	// themselves for the same reason — vitest browser mode's tests do not
	// each get their own realm, so a stub left standing here is a global
	// `fetch` some *other* file's test can inherit.
	vi.unstubAllGlobals();
});

test('dragging a widget by its header moves it and drops the body cursor class on pointer up', async () => {
	vi.stubGlobal(
		'fetch',
		vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
			const url = typeof input === 'string' ? input : input.toString();
			if (url.includes('/layout') && init?.method === 'PUT') {
				const body = JSON.parse(init.body as string) as { workspace_id?: string; widgets: unknown[] };
				return jsonResponse({
					workspace: { id: 'ws1', owner_id: 'u1', name: 'Default', columns: 6, revision: 2, created_at: 0, updated_at: 0 },
					widgets: body.widgets
				});
			}
			throw new Error(`unexpected fetch in a drag test: ${url}`);
		})
	);

	const layout = makeLayout();
	await render(DashboardGrid, { layout, catalog });

	function gridColumnStart(): number {
		const el = document.querySelector('[data-widget-id="w1"]');
		if (!el) throw new Error('widget cell w1 not found');
		// Chromium serializes an inline `grid-column`/`grid-row` pair back to
		// the attribute as the equivalent `grid-area` shorthand rather than
		// echoing the literal text this component wrote — reading the
		// computed style is what actually reflects what got applied.
		return Number(getComputedStyle(el).gridColumnStart);
	}
	const startColumn = gridColumnStart();

	const gridBox = document.querySelector('[data-dashboard-grid]')?.getBoundingClientRect();
	if (!gridBox) throw new Error('the dashboard grid has no box to measure');
	const gapPx = 12;
	const columns = 6;
	const cellWidthPx = (gridBox.width - gapPx * (columns - 1)) / columns;

	const handle = page.getByRole('button', { name: /^Move senken\/equity$/ });
	const handleEl = handle.element();
	const rect = handleEl.getBoundingClientRect();
	const startX = rect.x + rect.width / 2;
	const startY = rect.y + rect.height / 2;

	dragBy(handleEl, startX, startY, cellWidthPx + 8, 0);

	await expect.poll(gridColumnStart).toBe(startColumn + 1);

	expect(document.body.classList.contains('cursor-grabbing')).toBe(false);
	expect(document.body.classList.contains('select-none')).toBe(false);
});

test('the body cursor class is present while the drag is in progress', async () => {
	vi.stubGlobal(
		'fetch',
		vi.fn(async () => jsonResponse({}))
	);

	const layout = makeLayout();
	await render(DashboardGrid, { layout, catalog });

	const handle = page.getByRole('button', { name: /^Move senken\/equity$/ });
	const handleEl = handle.element();
	const rect = handleEl.getBoundingClientRect();
	const startX = rect.x + rect.width / 2;
	const startY = rect.y + rect.height / 2;
	const pointerId = 1;

	handleEl.dispatchEvent(
		new PointerEvent('pointerdown', { bubbles: true, clientX: startX, clientY: startY, pointerId, isPrimary: true })
	);
	expect(document.body.classList.contains('cursor-grabbing')).toBe(true);
	expect(document.body.classList.contains('select-none')).toBe(true);

	handleEl.dispatchEvent(
		new PointerEvent('pointerup', { bubbles: true, clientX: startX, clientY: startY, pointerId, isPrimary: true })
	);
	expect(document.body.classList.contains('cursor-grabbing')).toBe(false);
});

// A `senken/position-size` widget, not `senken/equity`/`senken/risk` —
// unlike those three, its own body actually reflects `config` visibly
// (`PositionSizeCard` reads it into its balance/risk/price inputs at
// mount), which is what lets "Undo restores the same config" be proven by
// reading the DOM rather than trusting the removal/reinsertion logic by
// description.
function layoutWithPositionSizeWidget(config: string): DashboardLayoutDto {
	return {
		workspace: { id: 'ws1', owner_id: 'u1', name: 'Default', columns: 6, revision: 1, created_at: 0, updated_at: 0 },
		widgets: [
			{
				id: 'w1',
				provider_id: 'senken',
				widget_type_id: 'senken/position-size',
				position_x: 0,
				position_y: 0,
				width: 4,
				height: 5,
				visible: true,
				config,
				config_schema_version: 1,
				created_at: 0,
				updated_at: 0
			}
		]
	};
}

test('undo restores a removed widget with its own config, and never reaches the server', async () => {
	const configJson = JSON.stringify({
		balance: '12345',
		riskPercent: '1',
		entryPrice: '100',
		stopPrice: '95',
		sizeDecimals: '2'
	});
	vi.stubGlobal(
		'fetch',
		vi.fn(async () => {
			throw new Error('undo must never let a save reach the server');
		})
	);

	// The removal toast's "Undo" button is rendered by `Toaster`, mounted
	// globally exactly once in the real app (`app-shell.svelte`) — this
	// isolated mount needs its own, or `toast(...)` calls have nothing to
	// render into and "Undo" never appears at all.
	await render(Toaster);
	const layout = layoutWithPositionSizeWidget(configJson);
	await render(DashboardGrid, { layout, catalog });

	await userEvent.click(page.getByRole('button', { name: /^Remove / }));
	expect(document.querySelector('[data-widget-id="w1"]')).toBeNull();

	await userEvent.click(page.getByRole('button', { name: 'Undo' }));

	await expect.poll(() => document.querySelector('[data-widget-id="w1"]')).not.toBeNull();
	const balanceInput = page.getByTestId('position-size-balance');
	await expect.element(balanceInput).toHaveValue('12345');
});

test('deleting without undo saves exactly once, after the undo window elapses', async () => {
	vi.useFakeTimers();
	const fetchMock = vi.fn(async (_input: RequestInfo | URL, init?: RequestInit) => {
		const body = JSON.parse(init?.body as string) as { widgets: unknown[] };
		return jsonResponse({
			workspace: { id: 'ws1', owner_id: 'u1', name: 'Default', columns: 6, revision: 2, created_at: 0, updated_at: 0 },
			widgets: body.widgets
		});
	});
	vi.stubGlobal('fetch', fetchMock);

	try {
		const layout = layoutWithPositionSizeWidget('{}');
		await render(DashboardGrid, { layout, catalog });

		document.querySelector<HTMLButtonElement>('[aria-label^="Remove "]')?.click();
		expect(fetchMock).not.toHaveBeenCalled();

		// The undo window (5 s) plus the debounced save it then schedules
		// (400 ms) — `advanceTimersByTimeAsync` so the `await fetch(...)`
		// inside `saveNow` actually gets to resolve between ticks, not just
		// the synchronous timer callbacks themselves.
		await vi.advanceTimersByTimeAsync(5000 + 400);

		expect(fetchMock).toHaveBeenCalledTimes(1);
	} finally {
		vi.useRealTimers();
	}
});

