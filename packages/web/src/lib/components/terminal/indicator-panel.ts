// Pure logic for the indicator-authoring panel: turning a compiled
// indicator's catalogue entry into the same `IndicatorCatalogItem` shape
// `+page.svelte`'s built-in indicator picker already builds
// (`$lib/charts/workspace-store.svelte.ts`), and working out what a
// re-compile must do to a pane's layer list so that changing one number
// and running again updates the existing plot rather than appending a
// second, orphaned one beside it.
//
// Kept free of `IndicatorPanel.svelte`'s own store/network calls
// (`addIndicatorLayer`/`removeLayer`/`apiClient`) so the property that
// actually matters here — recompiling never leaves a duplicate layer
// behind — is provable without mounting a chart or faking an HTTP
// response. `IndicatorPanel.svelte` calls `addIndicatorLayer`/`removeLayer`
// in the order this module works out, which is what keeps every series
// write on the chart's own already-guarded path
// (`chart-pane.svelte`'s `holdViewport`) rather than adding a second one.

import type { IndicatorCatalogEntry } from '$lib/api/types';
import type { IndicatorCatalogItem } from '$lib/charts/workspace-store.svelte';
import type { LayerRuntime } from '$lib/charts/pane-runtime';

/** A source new to the editor — the two-line shape every built-in
 * indicator's own doc string uses (`ema`/`plot`), so a trader opening this
 * panel for the first time sees a working example rather than a blank
 * box. */
export const DEFAULT_INDICATOR_SOURCE = 'let fast = ema(close, 5)\nplot fast\n';

/** Turns a freshly compiled `IndicatorCatalogEntry` (`POST
 * /api/indicators/compile`'s response) into the same
 * `IndicatorCatalogItem` shape the built-in indicator picker in
 * `+page.svelte` already builds from `GET /api/indicators`'s catalogue —
 * placing a compiled indicator on the chart goes through the exact same
 * `addIndicatorLayer` call a built-in does, never a second placement path. */
export function catalogItemFromEntry(entry: IndicatorCatalogEntry): IndicatorCatalogItem {
	return {
		name: entry.name,
		defaultParams: Object.fromEntries(entry.params.map((param) => [param.name, param.default.value])),
		placement: entry.placement
	};
}

/** After `addIndicatorLayer` resolves, the workspace store re-fetches the
 * pane's layers from the server, which mints a real id `addIndicatorLayer`
 * itself never learns (a `LayerInputDto` carries no id at all — see that
 * function's own module doc). This finds the one a compile-and-place just
 * added: among every layer named `indicatorName`, the one with the
 * highest `position` — a fresh append always lands last, which still
 * picks the right one even if an unrelated layer of the same indicator
 * was already on the pane before this panel ever ran. */
export function findJustPlacedLayer(layers: LayerRuntime[], indicatorName: string): LayerRuntime | null {
	const matches = layers.filter((l) => l.indicatorName === indicatorName);
	if (matches.length === 0) return null;
	return matches.reduce((latest, candidate) => (candidate.position > latest.position ? candidate : latest));
}

/** Models, purely, what one compile-and-place cycle does to a pane's own
 * layer list — exactly the sequence `IndicatorPanel.svelte` drives through
 * the real store (`removeLayer` then `addIndicatorLayer`): drop the layer
 * this panel placed on the *previous* run (if any), then append the
 * freshly compiled one. `newLayerId` stands in for the id the server
 * assigns on that real `addIndicatorLayer` call, which this pure function
 * never makes itself.
 *
 * This is the property that keeps a recompile from moving the chart's
 * viewport: every series write in `chart-pane.svelte` reacts to the
 * pane's layer *array*, wrapped in `holdViewport` — a recompile that
 * quietly grew that array (leaving the old attempt behind as a second,
 * orphaned plot) would still hold the viewport on each individual write,
 * but would leave two indicator series where the reader placed one, and
 * would re-trigger every other layer's own indicator fetch the moment the
 * array's signature changed for a reason that has nothing to do with
 * them. Replacing in place, one layer for one, is what avoids all of
 * that — proven below by recompiling twice and checking the layer count
 * never grows past one. */
export function applyRecompile(
	layers: LayerRuntime[],
	previousLayerId: string | null,
	item: IndicatorCatalogItem,
	newLayerId: string
): LayerRuntime[] {
	const withoutPrevious = previousLayerId ? layers.filter((l) => l.id !== previousLayerId) : layers;
	const placed: LayerRuntime = {
		id: newLayerId,
		position: withoutPrevious.length,
		kind: item.placement === 'sub_pane' ? 'indicator_sub_pane' : 'indicator_overlay',
		visible: true,
		indicatorName: item.name,
		params: { ...item.defaultParams }
	};
	return [...withoutPrevious, placed];
}

// "Save as belonging to the user; list self-authored indicators" is the
// indicator registry (`senken_indicator_registry::RegistryStore`, over
// `POST /api/registry/indicators` and `GET /api/registry/indicators/mine`)
// — `IndicatorPanel.svelte` calls `apiClient.publishIndicator`/
// `listMyIndicators`/`getRegistryIndicator` directly, the same way every
// other guarded listing in this app is called straight from a component
// rather than wrapped a second time here. There is nothing left to make
// pure in that path: unlike placing a layer on the chart, a publish is a
// single round trip with no local sequencing to get right.
