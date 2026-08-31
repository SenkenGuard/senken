// `GET /api/sources`, fetched once and shared for the life of this tab.
// A chart pane needs "does this venue stream a price" the moment it mounts,
// and the alerts panel needs the exact same answer for every alert in its
// list — both read this one cache rather than each issuing its own fetch.
//
// Mirrors `$lib/charts/workspace-error.ts`'s split: the actual decision
// logic (`hasLiveFeed`/`hasBarSource`) lives in the plain `source-capability.ts`,
// unit-tested there without the Svelte compiler; this file is only the thin
// rune-backed cache around one `fetch`.
import { apiClient } from './client';
import { getErrorMessage } from './errors';
import { hasBarSource as hasBarSourceOf, hasLiveFeed as hasLiveFeedOf } from './source-capability';
import type { SourceCapabilityDto } from './types';

class SourcesStore {
	byId = $state<Map<string, SourceCapabilityDto>>(new Map());
	loaded = $state(false);
	error = $state<string | null>(null);
}

export const sourcesStore = new SourcesStore();

let inflight: Promise<void> | null = null;

/** Fetches `GET /api/sources` exactly once per tab; every later caller
 * (a freshly mounted chart pane, the alerts panel opening) awaits the same
 * in-flight request instead of issuing its own. Safe to call from anywhere
 * that needs the cache warm — the first call wins, and a failure leaves
 * `loaded: false` so a later caller retries rather than caching the failure
 * forever. */
export function ensureSourcesLoaded(): Promise<void> {
	if (sourcesStore.loaded) return Promise.resolve();
	if (!inflight) {
		inflight = apiClient
			.listSources()
			.then((response) => {
				sourcesStore.byId = new Map(response.sources.map((s) => [s.id, s]));
				sourcesStore.loaded = true;
				sourcesStore.error = null;
			})
			.catch((error: unknown) => {
				sourcesStore.error = getErrorMessage(error, 'Could not load source capabilities.');
			})
			.finally(() => {
				inflight = null;
			});
	}
	return inflight;
}

/** Whether `instrument`'s source has a live feed pool — `undefined` while
 * `GET /api/sources` has not resolved yet, so a caller (see
 * `$lib/charts/live-state.ts`'s `deriveLiveState`) can tell "not known yet"
 * apart from "known to have none". */
export function hasLiveFeed(instrument: string): boolean | undefined {
	return sourcesStore.loaded ? hasLiveFeedOf(sourcesStore.byId, instrument) : undefined;
}

/** Whether `instrument`'s source has a bar source registered at all —
 * `undefined` while not yet loaded, same reasoning as `hasLiveFeed`. */
export function hasBarSource(instrument: string): boolean | undefined {
	return sourcesStore.loaded ? hasBarSourceOf(sourcesStore.byId, instrument) : undefined;
}
