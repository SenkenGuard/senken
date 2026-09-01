// Which settings sections are worth showing, as a plain function.
//
// Split out of `settings-registry.svelte.ts` for the same reason
// `$lib/charts/workspace-error.ts` was split out of its own store: a rune
// module cannot be imported by `bun test` at all (`$state` is a compiler
// construct, not a runtime one), and this rule is exactly the kind of thing
// that must be testable — it decides what a user can find in their own
// settings.

/** What the visibility flags are decided against. Passed in rather than read
 * here: whether the caller is an admin comes from `GET /api/me`, and whether
 * this is the desktop shell comes from the DOM. Neither belongs in a module
 * whose whole job is one boolean. */
export interface SettingsVisibilityContext {
	isAdmin: boolean;
	isDesktop: boolean;
}

/** Just the flags this rule reads. Structural rather than importing
 * `SettingsSection`, which lives in the rune module this file exists to stay
 * out of — a real section satisfies it without being told to. */
export interface SettingsSectionVisibility {
	adminOnly?: boolean;
	desktopOnly?: boolean;
}

/** Whether `section` belongs in this caller's settings at all.
 *
 * One predicate rather than a filter written at each call site, because
 * there are three — the nav list, the search results, and the modal's own
 * "which section am I rendering" lookup. A section hidden from one but not
 * the others is worse than showing it everywhere: search would offer a row
 * that jumps to a section the nav does not list.
 *
 * Cosmetic, exactly as `adminOnly`'s own doc says: an admin-only section's
 * content enforces its own check against the API and handles a 403. This
 * decides what is worth showing, never what is allowed.
 */
export function isSettingsSectionVisible(
	section: SettingsSectionVisibility,
	context: SettingsVisibilityContext
): boolean {
	if (section.adminOnly && !context.isAdmin) return false;
	if (section.desktopOnly && !context.isDesktop) return false;
	return true;
}
