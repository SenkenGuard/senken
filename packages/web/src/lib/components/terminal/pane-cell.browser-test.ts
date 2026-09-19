// Proves the fix for a real accessibility defect: `pane-cell.svelte`'s
// whole-pane wrapper used to carry `role="button"` while containing the
// chart plus every layer's own hide/settings/remove buttons — an
// interactive role nested around other interactive roles. Chromium's own
// accessibility tree (not jsdom, which never builds one) flattens that
// into a single control whose name is the concatenation of everything
// inside it, and prunes the nested buttons out of the tree entirely. That
// is why this lives in `*.browser-test.ts`: only a real accessibility
// tree, built by a real browser, can show the defect or its absence.
//
// Fixtures are the same recorded OKX-SPOT:BTCUSDT hourly bars
// `chart-pane.browser-test.ts` uses (see that file's own header for
// provenance) — `pane-cell.svelte` renders `chart-pane.svelte` internally,
// so the same network surface has to be stubbed here.
import { test, expect, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { page } from 'vitest/browser';
import PaneCell from './pane-cell.svelte';
import { defaultChartSettings } from '$lib/mock/chart-settings';
import type { LayerRuntime } from '$lib/charts/pane-runtime';

const BARS_PLAN_FIXTURE = {
	covered: [{ from: 1788228000000000000, to: 1788231600000000000 }],
	missing: [],
	chunks: 0,
	estimated_bars: 0,
	estimate_secs: null
};

const BARS_RANGE_FIXTURE = {
	bars: [
		{
			ts_open: 1788228000000000000,
			open: 784073,
			high: 784604,
			low: 781833,
			close: 784503,
			volume: { kind: 'real', value: 171982819550 },
			quote_volume: 13464028275030174,
			trade_count: null,
			taker_buy_volume: null
		}
	],
	missing: [],
	earliest_available: 1787900400000000000,
	price_scale: 1,
	qty_scale: 8,
	next_bar_open_at: 1788426000000000000
};

function jsonResponse(body: unknown): Response {
	return new Response(JSON.stringify(body), { status: 200, headers: { 'Content-Type': 'application/json' } });
}

/** Stubs every endpoint a mounting `chart-pane.svelte` calls, so a test can
 * mount `PaneCell` (which owns one) without asserting on bars/indicator
 * traffic at all — `chart-pane.browser-test.ts` already covers that. */
function stubPaneFetch(): void {
	vi.stubGlobal(
		'fetch',
		vi.fn(async (input: RequestInfo | URL) => {
			const url = typeof input === 'string' ? input : input.toString();
			if (url.includes('/api/bars/plan')) return jsonResponse(BARS_PLAN_FIXTURE);
			if (url.includes('/api/bars/range')) return jsonResponse(BARS_RANGE_FIXTURE);
			if (url.includes('/api/indicators/compute')) {
				return jsonResponse({ display: [], missing: [], discarded_objects: 0, warmup_truncated: false });
			}
			if (url.includes('/api/indicators')) return jsonResponse([{ name: 'Ema' }, { name: 'Sma' }]);
			if (url.includes('/api/sources')) return jsonResponse({ sources: [] });
			throw new Error(`unexpected fetch in a pane-cell test: ${url}`);
		})
	);
}

/** Two overlay layers and one sub-pane layer — the same shapes the bug
 * report's own accessibility-tree dump showed collapsed into one button's
 * name ("… Ema 50 my/mvp-sma 14"). */
function testLayers(): LayerRuntime[] {
	return [
		{ id: 'l-ema', position: 0, kind: 'indicator_overlay', visible: true, indicatorName: 'Ema', params: { period: 50 } },
		{ id: 'l-sma', position: 1, kind: 'indicator_sub_pane', visible: true, indicatorName: 'Sma', params: { period: 14 } }
	];
}

function paneProps(onFocus: () => void) {
	return {
		instrument: 'okx-spot:BTCUSDT',
		spec: '1h',
		layers: testLayers(),
		drawings: [],
		settings: defaultChartSettings(),
		reloadToken: 0,
		resetToken: 0,
		selectedDrawingId: null,
		showFocusRing: false,
		tool: 'cursor' as const,
		replaying: false,
		replayCut: null,
		replayPct: '',
		clearToken: 0,
		onFocus,
		onToggleLayer: () => {},
		onOpenMainSettings: () => {},
		onOpenLayerSettings: () => {},
		onRemoveLayer: () => {},
		onCreateDrawing: () => {},
		onSelectDrawing: () => {},
		onToolConsumed: () => {}
	};
}

// This one holds a property worth keeping — every layer control carries its
// own distinct accessible name — but it does NOT catch the nesting defect:
// Chromium leaves a real nested `<button>` in the accessibility tree
// whatever role its ancestor claims, so it stays green even with
// `role="button"` restored on the wrapper. The two tests below are the ones
// that fall on that mutation.
test('every layer control carries its own accessible name', async () => {
	stubPaneFetch();
	const screen = await render(PaneCell, paneProps(() => {}));

	// One per real layer/instrument chip in the header, found the way a
	// screen reader user actually would — by role and its own name, never
	// by the `[aria-label$="settings"]` attribute selector the bug report
	// says every existing test was forced into.
	await expect.element(page.getByRole('button', { name: 'Hide Ema', exact: true })).toBeInTheDocument();
	await expect.element(page.getByRole('button', { name: 'Ema settings', exact: true })).toBeInTheDocument();
	await expect.element(page.getByRole('button', { name: 'Remove Ema', exact: true })).toBeInTheDocument();
	await expect.element(page.getByRole('button', { name: 'Hide Sma', exact: true })).toBeInTheDocument();
	await expect.element(page.getByRole('button', { name: 'Sma settings', exact: true })).toBeInTheDocument();
	await expect.element(page.getByRole('button', { name: 'Remove Sma', exact: true })).toBeInTheDocument();

	// The main instrument chip's own settings button, distinct from every
	// layer's — proving the accessible tree still tells chips apart from
	// each other, not just from the pane.
	await expect.element(page.getByRole('button', { name: 'BTCUSDT settings', exact: true })).toBeInTheDocument();

	// The price-scale shortcuts, also individually addressable.
	await expect.element(page.getByRole('button', { name: 'Auto-fit price scale', exact: true })).toBeInTheDocument();
	await expect.element(page.getByRole('button', { name: 'Logarithmic price scale', exact: true })).toBeInTheDocument();

	// None of those names leak into each other or accumulate the whole
	// pane's text — each one is `getByRole`-unique on its own.
	expect(page.getByRole('button', { name: 'Hide Ema', exact: true }).elements().length).toBe(1);

	await screen.unmount();
	vi.unstubAllGlobals();
});

test('no element claiming to be a button contains another interactive descendant', async () => {
	stubPaneFetch();
	const screen = await render(PaneCell, paneProps(() => {}));

	// `role="button"` (native `<button>` or an explicit ARIA one) forbids
	// interactive descendants — that invariant is what this asserts
	// directly against the rendered DOM, not against source text.
	const buttonLikeElements = screen.container.querySelectorAll('button, [role="button"]');
	expect(buttonLikeElements.length).toBeGreaterThan(0);
	for (const button of buttonLikeElements) {
		const nestedInteractive = button.querySelector(
			'button, [role="button"], a[href], input, select, textarea, [tabindex]'
		);
		expect(nestedInteractive, `${button.outerHTML.slice(0, 120)} has an interactive descendant`).toBeNull();
	}

	await screen.unmount();
	vi.unstubAllGlobals();
});

test('the pane becomes the active pane whether it is reached with a mouse or the keyboard', async () => {
	stubPaneFetch();
	const onFocus = vi.fn();
	const screen = await render(PaneCell, paneProps(onFocus));

	const pane = page.getByRole('group', { name: /OKX-SPOT:BTCUSDT 1h chart pane/ });
	await expect.element(pane).toBeInTheDocument();

	// A mouse press on the pane's own background — not on any button —
	// activates it. `pointerdown` rather than `click`, so the pane is
	// already active by the time a drag on the chart begins.
	await pane.click({ position: { x: 5, y: 5 } });
	expect(onFocus).toHaveBeenCalled();

	// A press on a layer's own control activates the pane it lives in too;
	// the event bubbling that does this is deliberate.
	onFocus.mockClear();
	await page.getByRole('button', { name: 'Hide Ema', exact: true }).click();
	expect(onFocus).toHaveBeenCalled();

	// The keyboard route: the pane is not a tab stop of its own, because
	// there is nothing to activate on it — every control a keyboard user
	// wants is inside it. Focus entering any of them activates the pane,
	// so tabbing in is enough and no extra keystroke on the wrapper is
	// needed.
	onFocus.mockClear();
	expect(pane.element().hasAttribute('tabindex')).toBe(false);
	(page.getByRole('button', { name: 'Ema settings', exact: true }).element() as HTMLElement).focus();
	await expect.poll(() => onFocus.mock.calls.length).toBeGreaterThan(0);

	await screen.unmount();
	vi.unstubAllGlobals();
});

