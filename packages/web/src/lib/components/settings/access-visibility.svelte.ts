// Whether the current account should even see the "Users & Roles" settings
// section.
//
// This is cosmetic only, never enforcement ("hiding UI is not access
// control... enforcement is server-side, on every endpoint, always") —
// every request `access-section.svelte` makes still gets checked for real
// by `senken_identity::IdentityStore` regardless of what this module says.
// It exists purely so an ordinary account is not shown a section full of
// controls that will only ever 403.
//
// `GET /api/me`'s `roles`/`grants` fields are the first
// honest signal this app has had for that: before them, the only options
// were showing the section to everyone (what Q6 shipped, correctly, per
// its own header comment) or a fabricated heuristic like matching the
// seeded `admin@mail.com` address — which would *look* like a permission
// check without being one. This module reads the real thing instead.
import { apiClient } from '$lib/api/client';
import type { MeResponse } from '$lib/api/types';

class AccessVisibilityStore {
	/** The caller's own profile, or `null` before the first load completes
	 * or after one fails. `null` is treated identically to "no grant" by
	 * `canSeeAccessSection` below, so a transient fetch failure hides the
	 * section rather than mistakenly showing it. */
	profile = $state<MeResponse | null>(null);
}

export const accessVisibility = new AccessVisibilityStore();

/** Loads (or reloads) the profile `canSeeAccessSection` reads. Safe to call
 * every time the settings modal opens — `GET /api/me` is a cheap,
 * already-authenticated call the heartbeat polls at the same cadence
 * anyway. Failures are swallowed: this signal only ever hides a UI
 * element, so there is nothing for a caller to react to beyond that. */
export async function loadAccessVisibility(): Promise<void> {
	try {
		accessVisibility.profile = await apiClient.me();
	} catch {
		accessVisibility.profile = null;
	}
}

/** `true` if `profile` carries any grant on `Resource::User` or
 * `Resource::Role`, at any `Action`/`Scope` — the narrowest honest reading
 * of "this account has some administrative capability over users or
 * roles" available from `MeResponse` today, and exactly the domain the
 * Access section manages. */
export function canSeeAccessSection(profile: MeResponse | null): boolean {
	if (!profile) return false;
	return profile.grants.some((grant) => grant.resource === 'User' || grant.resource === 'Role');
}
