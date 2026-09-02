// The signed-in user's display (timezone) zone, loaded once and read by
// every chart, axis and clock in the app.
//
// Same shape as `$lib/components/settings/access-visibility.svelte.ts`: a
// module-level `$state` singleton plus an exported `load...` function,
// reused here rather than inventing a second way to hold server-backed,
// app-wide user state. `GET /api/me/zone` is the source of truth
// (`account-section.svelte`'s own settings picker calls the same
// endpoint) — this module exists so a chart pane, which has no reason to
// know that endpoint exists, can still read the answer.
//
// `zone` is never unset. Before the server has answered, and whenever it
// reports no chosen zone yet, this holds the browser's own zone — the same
// one-time proposal the settings picker itself falls back to (see
// `$lib/time/zone.ts`'s own doc: a *default*, never a substitute source of
// truth once a real value exists) — so a chart never has to handle "no zone
// at all" as a rendering case.
import { apiClient } from '$lib/api/client';
import { detectBrowserZone } from '$lib/time';

const FALLBACK_ZONE = 'UTC';

/** `detectBrowserZone` only promises a meaningful answer in a browser tab
 * (see its own doc); this app is SPA-only (`ssr = false`,
 * `routes/+layout.ts`) so that is every context this module ever runs in,
 * but the fallback still guards the one environment that is neither a
 * browser tab nor SSR: a `bun test` process, where `Intl` exists but a
 * resolved zone is not guaranteed to be assignable-looking in every CI
 * image. */
function detectDefaultZone(): string {
	try {
		return detectBrowserZone();
	} catch {
		return FALLBACK_ZONE;
	}
}

class UserZoneStore {
	/** The zone every chart and clock should render in right now. */
	zone = $state<string>(detectDefaultZone());
}

export const userZoneStore = new UserZoneStore();

/** Loads (or reloads) the stored display zone from `GET /api/me/zone`.
 * Safe to call whenever a session is established — a failed or still-`null`
 * read just leaves `userZoneStore.zone` at the browser default it already
 * had, the same fallback the settings picker itself shows. */
export async function loadUserZone(): Promise<void> {
	try {
		const response = await apiClient.getZone();
		if (response.zone) userZoneStore.zone = response.zone;
	} catch {
		// Non-fatal: charts keep rendering in whatever zone they already had
		// (the browser default, or the last value successfully loaded).
	}
}

/** Applies a newly-saved zone immediately, without waiting for a reload —
 * the whole point of "pick a zone and charts follow it". Call this after
 * `apiClient.setZone` succeeds (see `account-section.svelte`), never as a
 * substitute for actually persisting the choice server-side. */
export function setUserZone(zone: string): void {
	userZoneStore.zone = zone;
}
