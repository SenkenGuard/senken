// The settings-section registry.
//
// B10 is explicit that this must be a registry, not a hardcoded array:
// "plugins will add sections, and a hardcoded list cannot be extended
// without surgery. Core registers its own sections through the same
// mechanism plugins will use, so the mechanism is exercised from the start
// rather than theorised." This module is that mechanism. Core's own five
// sections (`register-core-sections.ts`) call exactly the function a future
// plugin would call — there is no second, privileged registration path.
//
// Module-level rune store, the same pattern `$lib/state/command-palette.svelte.ts`
// and `$lib/api/servers.svelte.ts` already use for shell-wide singletons:
// the settings modal is mounted once (from `nav-rail.svelte`, the owner of
// its trigger), so a plain module import gives every future registrant
// (core sections today, a plugin's activation code later) access with no
// prop-drilling and no "used outside provider" risk.
import type { Component, Snippet } from 'svelte';

/** One row inside a settings group: label + description on the left,
 * a control on the right, and — per its "reset-to-default affordance on
 * rows that have been changed" — an optional live changed/reset pair. A
 * row with no `changed`/`onReset` simply never shows the affordance (an
 * action like "change password" has no persisted default to reset to). */
export interface SettingsRow {
	/** Stable within its section — used as the `#each` key and as the
	 * anchor id search results scroll to. */
	id: string;
	label: string;
	description: string;
	/** Renders the row's control (a switch, a native `<select>`, a button —
	 * B10: "controls are mixed"). A snippet, not a `Component`, so a
	 * section can close over its own local state without inventing a prop
	 * contract every control shape would have to fit. */
	control: Snippet;
	/** `true` while this row's live value differs from its default. A
	 * getter (not a plain boolean) because it must stay live while the
	 * section is mounted and the user is editing it. */
	changed?: () => boolean;
	/** Reverts this row to its default value. Present only alongside
	 * `changed`. */
	onReset?: () => void;
}

/** Rows grouped under one heading with a one-line description ("the
 * right pane is a scrolling list of settings grouped under a heading with a
 * one-line description"). */
export interface SettingsGroup {
	id: string;
	heading: string;
	description: string;
	rows: SettingsRow[];
}

/** A flattened, text-only projection of a section's rows, used purely for
 * cross-section search ("a search field above [the nav] that filters
 * across all sections... once plugins add sections, browsing alone stops
 * being enough to find a setting"). Kept separate from `SettingsGroup`
 * because search must be able to rank every registered section's rows
 * without mounting each section's component to find out what it renders. */
export interface SettingsSearchEntry {
	groupHeading: string;
	rowId: string;
	rowLabel: string;
	rowDescription: string;
}

export interface SettingsSection {
	/** Stable, namespaced like a plugin permission id would be — core's own
	 * sections use a bare name (`'account'`) since they have no namespace to
	 * collide with; a future plugin section should prefix its own id with
	 * its plugin id for the same reason B9 namespaces permission names. */
	id: string;
	label: string;
	icon: Component;
	/** The section's content, rendered in the right pane when it is
	 * selected. Receives no props — a section closes over whatever state
	 * (API calls, local stores) it needs itself, the same way a route
	 * component would. */
	component: Component;
	/** Static text index for search (see `SettingsSearchEntry`). Does not
	 * need to reflect live values — search finds a *setting by name*, not
	 * by its current value. */
	searchIndex: SettingsSearchEntry[];
	/** hiding a section is cosmetic only, never access
	 * control — the admin-only sections' own content is what must enforce
	 * a real check against the API and handle a `403` gracefully. This
	 * flag exists purely so the nav does not clutter every user's view
	 * with controls they cannot use; it must never be the only thing
	 * standing between a user and an admin action. */
	adminOnly?: boolean;
	/** Lower sorts first. Core sections use small integers with gaps so a
	 * plugin section can slot in between without renumbering everything
	 * else; unset sorts after every explicitly ordered section. */
	order?: number;
}

class SettingsRegistry {
	sections = $state<SettingsSection[]>([]);
}

export const settingsRegistry = new SettingsRegistry();

function bySortOrder(a: SettingsSection, b: SettingsSection): number {
	const ao = a.order ?? Number.MAX_SAFE_INTEGER;
	const bo = b.order ?? Number.MAX_SAFE_INTEGER;
	return ao - bo;
}

/** Registers a settings section. This is the *only* mechanism — core's own
 * sections (`register-core-sections.ts`) call it exactly like a plugin
 * would. Returns an unregister function so a caller that can be torn down
 * (a plugin deactivating) can remove its section again; core never calls
 * it since its sections live for the app's lifetime.
 *
 * Re-registering an id already present replaces it in place (keeps its
 * position) rather than duplicating — a plugin re-activating after a
 * reload should not accumulate copies of its own section. */
export function registerSettingsSection(section: SettingsSection): () => void {
	const existingIndex = settingsRegistry.sections.findIndex((s) => s.id === section.id);
	if (existingIndex === -1) {
		settingsRegistry.sections = [...settingsRegistry.sections, section].sort(bySortOrder);
	} else {
		const next = settingsRegistry.sections.slice();
		next[existingIndex] = section;
		settingsRegistry.sections = next.sort(bySortOrder);
	}
	return () => unregisterSettingsSection(section.id);
}

export function unregisterSettingsSection(id: string): void {
	settingsRegistry.sections = settingsRegistry.sections.filter((s) => s.id !== id);
}

/** All registered sections in display order. A thin wrapper so callers
 * don't reach into `settingsRegistry.sections` (already sorted, but the
 * indirection keeps the sort as this module's responsibility, not
 * something every reader re-derives). */
export function listSettingsSections(): SettingsSection[] {
	return settingsRegistry.sections;
}

export interface SettingsSearchResult {
	sectionId: string;
	sectionLabel: string;
	entry: SettingsSearchEntry;
}

/** Cross-section search. Case-insensitive substring match over each
 * row's label, description and parent group heading — deliberately simple:
 * the point B10 makes is that search must reach *every* section, not that
 * it needs fuzzy ranking. */
export function searchSettings(query: string): SettingsSearchResult[] {
	const q = query.trim().toLowerCase();
	if (q === '') return [];
	const results: SettingsSearchResult[] = [];
	for (const section of settingsRegistry.sections) {
		for (const entry of section.searchIndex) {
			const haystack = `${entry.groupHeading} ${entry.rowLabel} ${entry.rowDescription}`.toLowerCase();
			if (haystack.includes(q)) {
				results.push({ sectionId: section.id, sectionLabel: section.label, entry });
			}
		}
	}
	return results;
}
