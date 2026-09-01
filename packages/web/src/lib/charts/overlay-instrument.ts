// Pure decisions behind plotting a second instrument on a pane
// (`addInstrumentOverlay` -> an `overlay_instrument` layer). Split out of
// `chart-pane.svelte` so the reconciliation logic — which layers need a
// series, under which isolated price-scale id, and how a raw bar becomes a
// chart point — can be proven without a lightweight-charts instance.
import type { LayerRuntime } from './pane-runtime';

/** The price-scale id one `overlay_instrument` layer's line series is pinned
 * to. Kept one-per-layer (never shared, unlike the volume overlay's single
 * `'senken-volume-overlay'`) because two overlay instruments can themselves
 * sit at wildly different prices — a shared scale would fight over the same
 * axis the same way sharing the candles' scale would. The scale itself is
 * left invisible: it exists only so the series autoscales independently,
 * never to add a second labelled axis. */
export function overlayInstrumentPriceScaleId(layerId: string): string {
	return `senken-overlay-${layerId}`;
}

/** One overlay-instrument layer's plotting plan: which layer, which
 * instrument, whether it is currently visible, and the price-scale id its
 * series draws on. */
export interface OverlayInstrumentPlan {
	layerId: string;
	instrument: string;
	visible: boolean;
	priceScaleId: string;
}

/** Which of this pane's layers need an overlay-instrument series, and under
 * which price-scale id. Includes hidden layers (`visible: false` in the
 * result) — a caller reconciling against this keeps a hidden layer's series
 * and only flips its `visible` option, rather than tearing it down and
 * rebuilding it, the same "an option flip, not a teardown" choice
 * `chart-pane.svelte`'s indicator-overlay effect already makes for its own
 * series. A layer that is gone (removed, or an `overlay_instrument` layer
 * somehow missing its `instrument`) simply has no entry here, so
 * `overlaySeriesKeysToRemove` below can tell a caller to tear its series
 * down. */
export function planOverlayInstrumentSeries(layers: LayerRuntime[]): OverlayInstrumentPlan[] {
	return layers
		.filter((layer) => layer.kind === 'overlay_instrument' && !!layer.instrument)
		.map((layer) => ({
			layerId: layer.id,
			instrument: layer.instrument as string,
			visible: layer.visible,
			priceScaleId: overlayInstrumentPriceScaleId(layer.id)
		}));
}

/** A value the reconciliation effect can track as its one dependency instead
 * of the raw `overlayLayers` array — whose identity changes on every parent
 * re-render (the parent re-renders on every live tick), which would re-run
 * the effect, and its network fetches, several times a second. Mirrors
 * `chart-pane.svelte`'s existing `overlaySignature` for indicator layers. */
export function overlayInstrumentSignature(layers: LayerRuntime[]): string {
	return planOverlayInstrumentSeries(layers)
		.map((plan) => `${plan.layerId}:${plan.instrument}:${plan.visible}`)
		.join('|');
}

/** Series keys a caller currently holds (its own `Map`'s keys) that no
 * planned layer claims any more — what it must tear down after reconciling.
 * A layer that is merely hidden still appears in `plan`, so it is never
 * returned here; only an actually removed layer is. */
export function overlaySeriesKeysToRemove(
	plan: OverlayInstrumentPlan[],
	existingKeys: Iterable<string>
): string[] {
	const wanted = new Set(plan.map((p) => p.layerId));
	return [...existingKeys].filter((key) => !wanted.has(key));
}

/** One overlay instrument's bar, as the raw scaled integers `loadBars`
 * resolves — the shape this module needs, not the whole `BarDto`. */
export interface OverlayInstrumentBar {
	ts_open: number;
	close: number;
}

/** One converted chart point. */
export interface OverlayInstrumentPoint {
	time: number;
	value: number;
}

/** Converts an overlay instrument's raw closes into decimal chart points,
 * scaled by *that instrument's own* `priceScale` — from its own `loadBars`
 * response, never the pane's main instrument's. The two scales are
 * unrelated: each instrument independently picks how many decimal places
 * its integer prices represent, so applying the wrong one does not shift
 * the line a little — it plots a price off by orders of magnitude. */
export function overlayInstrumentCloseSeries(
	bars: OverlayInstrumentBar[],
	overlayPriceScale: number
): OverlayInstrumentPoint[] {
	const divisor = 10 ** overlayPriceScale;
	return bars.map((bar) => ({
		time: Math.floor(bar.ts_open / 1_000_000_000),
		value: bar.close / divisor
	}));
}
