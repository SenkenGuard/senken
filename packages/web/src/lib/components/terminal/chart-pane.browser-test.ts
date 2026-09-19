// Interactive proof for the bug `AGENTS.md` calls "derive from what you
// hold, not from what you opened with": `chart-pane.svelte`'s bar-loading
// effect reads the tracked `bars` array (`reloadWindow(bars, …)`) and then,
// in its own `.then`, writes a brand-new `bars` array — which re-triggers
// the very effect that wrote it. Nothing in the existing 64 `bun test`
// files can see this: every one of them calls a pure function once and
// checks its return value, never mounts a live reactive scope across
// several ticks.
//
// This is the one component in the pane that still owns that effect
// inline — the fixture below is deliberately real: recorded 2026-09-03 by
// running `./target/debug/senken serve` against a copy of this repo's own
// `.data` and `curl`ing `GET /api/bars/plan` and `GET /api/bars/range` for
// `okx-spot:BTCUSDT`/`1h`, never hand-typed — a hand-written DTO would not
// have caught a shape this test does not even exercise.
import { test, expect, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import ChartPane from './chart-pane.svelte';
import { defaultChartSettings } from '$lib/mock/chart-settings';
import { refreshIndicatorCatalog } from '$lib/charts/indicator-catalog.svelte';
import type { LayerRuntime } from '$lib/charts/pane-runtime';

// Recorded verbatim from `GET /api/bars/plan?instrument=okx-spot:BTCUSDT&spec=1h&from=1788228000000000000&to=1788238800000000000`.
const BARS_PLAN_FIXTURE = {
	covered: [
		{ from: 1788228000000000000, to: 1788231600000000000 },
		{ from: 1788231600000000000, to: 1788235200000000000 },
		{ from: 1788235200000000000, to: 1788238800000000000 }
	],
	missing: [],
	chunks: 0,
	estimated_bars: 0,
	estimate_secs: null
};

// Recorded verbatim from `GET /api/bars/range` for the same instrument/spec/range — three real OKX-SPOT:BTCUSDT hourly bars.
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
		},
		{
			ts_open: 1788231600000000000,
			open: 784503,
			high: 788726,
			low: 784001,
			close: 786823,
			volume: { kind: 'real', value: 115858898950 },
			quote_volume: 9111782796429696,
			trade_count: null,
			taker_buy_volume: null
		},
		{
			ts_open: 1788235200000000000,
			open: 786824,
			high: 789361,
			low: 786501,
			close: 787295,
			volume: { kind: 'real', value: 109156030170 },
			quote_volume: 8596585012927727,
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

test('a chart left alone stops fetching after its first load', async () => {
	const callCounts = { plan: 0, range: 0 };
	vi.stubGlobal(
		'fetch',
		vi.fn(async (input: RequestInfo | URL) => {
			const url = typeof input === 'string' ? input : input.toString();
			if (url.includes('/api/bars/plan')) {
				callCounts.plan += 1;
				return jsonResponse(BARS_PLAN_FIXTURE);
			}
			if (url.includes('/api/bars/range')) {
				callCounts.range += 1;
				return jsonResponse(BARS_RANGE_FIXTURE);
			}
			if (url.includes('/api/indicators/compute')) {
				return jsonResponse({ display: [], missing: [], discarded_objects: 0, warmup_truncated: false });
			}
			if (url.includes('/api/sources')) {
				return jsonResponse({ sources: [] });
			}
			throw new Error(`unexpected fetch in an idle-chart test: ${url}`);
		})
	);

	const screen = await render(ChartPane, {
		instrument: 'okx-spot:BTCUSDT',
		spec: '1h',
		tool: 'cursor',
		replayIdx: null,
		clearToken: 0,
		overlayLayers: [],
		drawings: [],
		selectedDrawingId: null,
		settings: defaultChartSettings()
	});

	// Let the initial load resolve and whatever it triggers next settle.
	await new Promise((resolve) => setTimeout(resolve, 1000));
	const plansAtOneSecond = callCounts.plan;

	await new Promise((resolve) => setTimeout(resolve, 500));
	const plansAtOnePointFiveSeconds = callCounts.plan;

	// The property this test exists for: a pane nobody touches must stop
	// asking. The effect used to read `bars` and then overwrite it in its own
	// `.then`, so it re-triggered itself forever — measured at over twenty
	// thousand plan requests inside this one-second window. The equality below
	// is what proves that is gone: whatever startup asked for, it asked once
	// and stopped.
	//
	// The count itself is bounded rather than pinned to one, and deliberately
	// so. `loadBars` issues exactly one plan request per call, but a mounting
	// pane makes more than one call: the visible window, plus the history
	// pager's own prefetch of the pages either side of it. Three is what that
	// costs today. Pinning this at one or two would not describe the loader —
	// it would just fail whenever the prefetch happened to land inside the
	// window, which is what it did before this bound was measured rather than
	// assumed.
	expect(plansAtOneSecond).toBeLessThanOrEqual(4);
	expect(plansAtOnePointFiveSeconds).toBe(plansAtOneSecond);

	await screen.unmount();
	vi.unstubAllGlobals();
});

function smaLayer(overrides: Partial<LayerRuntime> = {}): LayerRuntime {
	return {
		id: 'l1',
		position: 0,
		kind: 'indicator_overlay',
		visible: true,
		indicatorName: 'Sma',
		params: { period: 14 },
		...overrides
	};
}

/** Same fixtures as the idle-chart test above, plus `/api/indicators` (the
 * shared catalogue poll every pane now subscribes to) and a per-endpoint
 * call count, so a test can assert on `/api/indicators/compute` without
 * also having to special-case the bars/catalogue traffic every load already
 * causes. */
function stubComputeGateFetch(counts: { compute: number; catalog: number }): void {
	vi.stubGlobal(
		'fetch',
		vi.fn(async (input: RequestInfo | URL) => {
			const url = typeof input === 'string' ? input : input.toString();
			if (url.includes('/api/bars/plan')) return jsonResponse(BARS_PLAN_FIXTURE);
			if (url.includes('/api/bars/range')) return jsonResponse(BARS_RANGE_FIXTURE);
			if (url.includes('/api/indicators/compute')) {
				counts.compute += 1;
				return jsonResponse({ display: [], missing: [], discarded_objects: 0, warmup_truncated: false });
			}
			if (url.includes('/api/indicators')) {
				counts.catalog += 1;
				return jsonResponse([{ name: 'Sma' }]);
			}
			if (url.includes('/api/sources')) return jsonResponse({ sources: [] });
			throw new Error(`unexpected fetch in a compute-gate test: ${url}`);
		})
	);
}

test('an overlay is computed once per loaded range, not once per render', async () => {
	const counts = { compute: 0, catalog: 0 };
	stubComputeGateFetch(counts);

	const screen = await render(ChartPane, {
		instrument: 'okx-spot:BTCUSDT',
		spec: '1h',
		tool: 'cursor',
		replayIdx: null,
		clearToken: 0,
		overlayLayers: [smaLayer()],
		drawings: [],
		selectedDrawingId: null,
		settings: defaultChartSettings()
	});

	// Bars resolve; the shared catalogue starts empty and arrives moments
	// later (its own fetch), and only once it does does the layer pass the
	// catalogue filter and actually get computed.
	await new Promise((resolve) => setTimeout(resolve, 1000));
	const computedAfterLoad = counts.compute;
	expect(computedAfterLoad).toBe(1);

	// Forces a second, redundant catalogue poll — the exact trigger the gate
	// exists for: a fresh `Set` with identical contents, which used to be
	// enough on its own to re-run this effect and recompute, even though
	// neither the loaded range nor the layer set had moved at all.
	refreshIndicatorCatalog();
	await new Promise((resolve) => setTimeout(resolve, 1000));
	expect(counts.compute).toBe(computedAfterLoad);

	await screen.unmount();
	vi.unstubAllGlobals();
});

test('a sub-pane indicator is computed once per loaded range, not once per render', async () => {
	const counts = { compute: 0, catalog: 0 };
	stubComputeGateFetch(counts);

	const screen = await render(ChartPane, {
		instrument: 'okx-spot:BTCUSDT',
		spec: '1h',
		tool: 'cursor',
		replayIdx: null,
		clearToken: 0,
		overlayLayers: [],
		subPaneLayers: [smaLayer({ id: 'l2', kind: 'indicator_sub_pane' })],
		drawings: [],
		selectedDrawingId: null,
		settings: defaultChartSettings()
	});

	await new Promise((resolve) => setTimeout(resolve, 1000));
	const computedAfterLoad = counts.compute;
	expect(computedAfterLoad).toBe(1);

	// Same trigger as the overlay test above, aimed at the native-pane
	// reconciliation effect — which, unlike the overlay effect, used to have
	// no gate of any kind: every re-run recomputed unconditionally, spinner
	// or not.
	refreshIndicatorCatalog();
	await new Promise((resolve) => setTimeout(resolve, 1000));
	expect(counts.compute).toBe(computedAfterLoad);

	await screen.unmount();
	vi.unstubAllGlobals();
});
