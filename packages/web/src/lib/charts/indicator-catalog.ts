// The set of indicator names a chart may currently place — the ten
// built-ins `senken-indicators` ships with, plus every dynamic indicator
// loaded from an uploaded `.wasm` component that has not been disabled.
//
// `chart-pane.svelte` uses this to gate its two batch indicator-compute
// effects: a layer naming something outside this set is left out of the
// fetch entirely rather than sent to `POST /api/indicators/compute` (which
// would 400 for an unknown or disabled dynamic indicator, and — worse, if
// every layer shared one `Promise.all` — would also block every other
// layer's own reconciliation on the same rejected promise). Excluding it is
// what turns "disabled" into a placeholder: the layer itself is never
// touched (its `indicatorName`/`params` live in the workspace's own layer
// state, entirely outside this module), only its plot disappears, through
// the same "not seen -> removed" cleanup a deleted layer already goes
// through.

import { apiClient } from '$lib/api/client';

/** Fetches the current catalogue and reduces it to the bare set of names a
 * layer's own `indicatorName` is compared against. Throws (network error,
 * non-OK response) exactly like `apiClient.listIndicators` does — a caller
 * decides what "the catalogue could not be read right now" means for it
 * (`chart-pane.svelte` keeps whatever set it already had rather than
 * treating a transient failure as "every dynamic indicator was just
 * disabled"). */
export async function fetchIndicatorCatalog(): Promise<Set<string>> {
	const entries = await apiClient.listIndicators();
	return new Set(entries.map((entry) => entry.name));
}
