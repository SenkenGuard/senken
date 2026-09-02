// The browser's own IANA zone — used only as a one-time seed, never as an
// implicit source of truth.
//
// A user's display zone is stored on the server precisely so the same
// person sees the same chart on their phone and their desktop, and so a
// server-rendered report does not depend on whichever machine happened to
// render it (see `GET`/`PUT /api/me/zone`, backed by
// `crates/api/src/dto/time.rs`'s `UserZoneResponse`/`SetZoneRequest` and
// `$lib/api/client.ts`'s `getZone`/`setZone`). The browser's own zone
// matters exactly once: as the value this app proposes the first time a
// user has not chosen a display zone yet (see
// `$lib/components/settings/sections/account-section.svelte`).

/** The IANA zone the browser itself reports, e.g. `'Asia/Jakarta'`.
 *
 * Call this only to *propose* a default the first time a user's display
 * zone has not been set — never to decide how an already-set zone behaves,
 * and never on every request as a substitute for the stored value. There is
 * also no `window`/`Intl` locale-database guarantee on the server, so this
 * function has no meaning called from anywhere but a browser tab. */
export function detectBrowserZone(): string {
	return Intl.DateTimeFormat().resolvedOptions().timeZone;
}
