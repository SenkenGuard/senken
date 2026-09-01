// Which settings sections are worth showing, as a plain function.
//
// Split out of `settings-registry.svelte.ts` for the same reason
// `$lib/charts/workspace-error.ts` was split out of its own store: a rune
// module cannot be imported by `bun test` at all (`$state` is a compiler
// construct, not a runtime one), and this rule is exactly the kind of thing
// that must be testable — it decides what a user can find in their own
// settings.

/** What the visibility rules are decided against. Passed in rather than read
 * here: which resources the caller holds a grant on comes from
 * `GET /api/me`, and whether this is the desktop shell comes from the DOM.
 * Neither belongs in a module whose whole job is one boolean. */
export interface SettingsVisibilityContext {
	/** Every `Resource` the caller holds any grant on, at any action or
	 * scope — the honest reading of "has some administrative capability
	 * over this" that `MeResponse` supports. */
	grantedResources: readonly string[];
	isDesktop: boolean;
}

/** Just what this rule reads. Structural rather than importing
 * `SettingsSection`, which lives in the rune module this file exists to stay
 * out of — a real section satisfies it without being told to. */
export interface SettingsSectionVisibility {
	/** The resources this section administers. Shown when the caller holds
	 * a grant on **any** of them.
	 *
	 * Named for the resources rather than as a plain "admin only" flag
	 * because there is no single administrator: an account can be granted
	 * storage without users, or the reverse. Collapsing both into one
	 * boolean meant a section was shown or hidden on the strength of a
	 * permission over something else entirely. */
	requiresAnyResource?: readonly string[];
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
 * Cosmetic, always: a restricted section's content enforces its own check
 * against the API and handles a 403. This decides what is worth showing,
 * never what is allowed.
 */
export function isSettingsSectionVisible(
	section: SettingsSectionVisibility,
	context: SettingsVisibilityContext
): boolean {
	const required = section.requiresAnyResource;
	if (required && required.length > 0) {
		if (!required.some((resource) => context.grantedResources.includes(resource))) return false;
	}
	if (section.desktopOnly && !context.isDesktop) return false;
	return true;
}
