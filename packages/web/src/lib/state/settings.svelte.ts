// Global settings-modal overlay state.
//
// Same architectural call as `$lib/state/command-palette.svelte.ts`: a
// module-level rune store rather than Svelte context, so any leaf button —
// the nav-rail gear icon, a future "manage users" link elsewhere — can open
// the modal by importing this file directly. See that file's header
// comment for the full reasoning; it applies unchanged here.
export type SettingsOpenOptions = {
	/** Section id to land on. Falls back to the first registered section
	 * (by `order`) when omitted or when the id no longer resolves — e.g. a
	 * bookmarked admin section id for a user who has since lost the admin
	 * section from view. */
	sectionId?: string;
};

class SettingsModalStore {
	open = $state(false);
	activeSectionId = $state<string | null>(null);
	query = $state('');
	/** Set by picking a cross-section search result so the content
	 * pane can scroll that row into view and briefly highlight it once its
	 * section is mounted. Cleared by whatever consumes it — see
	 * `settings-modal.svelte`'s `$effect`. */
	pendingHighlightRowId = $state<string | null>(null);
}

export const settingsModal = new SettingsModalStore();

export function openSettings(options: SettingsOpenOptions = {}): void {
	if (options.sectionId) settingsModal.activeSectionId = options.sectionId;
	settingsModal.query = '';
	settingsModal.open = true;
}

export function closeSettings(): void {
	settingsModal.open = false;
	settingsModal.query = '';
}

export function selectSettingsSection(id: string): void {
	settingsModal.activeSectionId = id;
	settingsModal.query = '';
}

/** Jump to a specific row found via search: selects its section, clears
 * the query, and arms the highlight `settings-modal.svelte` reacts to. */
export function jumpToSettingsRow(sectionId: string, rowId: string): void {
	settingsModal.activeSectionId = sectionId;
	settingsModal.query = '';
	settingsModal.pendingHighlightRowId = rowId;
}
