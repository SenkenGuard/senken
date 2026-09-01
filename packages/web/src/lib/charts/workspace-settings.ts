// Converts a chart workspace's display settings to and from
// `WorkspaceDto.settings`/the `PATCH /api/workspaces/{id}/settings` body —
// opaque JSON-object text the server stores and returns without
// interpreting (its own doc: "the chart front end owns what these settings
// mean"). Same shape and same reasoning as `pane-settings.ts`'s
// `parsePaneSettings`/`paneSettingsToJson`, one level up: a pane's settings
// describe how one chart looks, a workspace's describe which of the user's
// own watchlist groups and notes are shown in it.
//
// A workspace nobody has configured carries `"{}"` — the same documented
// empty case `pane-settings.ts` handles — and must resolve to an empty
// watchlist and an empty notes panel, not every group/note the user owns.
export interface ChartWorkspaceSettings {
	/** Ids of the watchlist groups shown in this workspace's watchlist panel. */
	watchlistGroups: string[];
	/** Ids of the notes shown in this workspace's notes panel. */
	notes: string[];
}

export function defaultWorkspaceSettings(): ChartWorkspaceSettings {
	return { watchlistGroups: [], notes: [] };
}

/** Parses `json` into a full `ChartWorkspaceSettings`, tolerating both the
 * documented empty case (`"{}"`) and anything malformed — a workspace's
 * settings text is never allowed to break the panels it configures, so a
 * parse failure or a non-object value falls back to
 * `defaultWorkspaceSettings()` entirely rather than throwing or handing back
 * a partially-`undefined` object. */
export function parseWorkspaceSettings(json: string): ChartWorkspaceSettings {
	const base = defaultWorkspaceSettings();
	let parsed: unknown;
	try {
		parsed = JSON.parse(json);
	} catch {
		// Malformed settings text — the workspace's panels still render with
		// every default (nothing shown) rather than being lost along with
		// the bad value.
		return base;
	}
	if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return base;
	const stored = parsed as Record<string, unknown>;
	const merged: Record<string, unknown> = { ...base };
	// Field by field, and only the fields this build has — see
	// `pane-settings.ts`'s `parsePaneSettings` for why a blanket spread is
	// wrong twice over: a key this build no longer has would ride along in
	// the object and be written straight back out on the next save, and a
	// value of the wrong type would reach the panels as-is. Both fields here
	// are arrays of ids, so the one check that matters is "array of
	// strings", not "array of numbers" like `pane-settings.ts`'s `paneSplit`.
	for (const key of Object.keys(base)) {
		const value = stored[key];
		if (Array.isArray(value) && value.every((entry) => typeof entry === 'string')) {
			merged[key] = value;
		}
	}
	return merged as unknown as ChartWorkspaceSettings;
}

/** The inverse of `parseWorkspaceSettings` — plain `JSON.stringify`, kept as
 * its own named function so both directions of the codec live next to each
 * other, matching `pane-settings.ts`'s `paneSettingsToJson`. */
export function workspaceSettingsToJson(settings: ChartWorkspaceSettings): string {
	return JSON.stringify(settings);
}

/**
 * Which of a user's own rows (watchlist groups, notes — anything with a
 * stable `id`) are shown in a workspace, given that workspace's stored ids
 * and the full list the user owns.
 *
 * The order comes from `owned`, the owning list, never from `shownIds` —
 * `shownIds` is an unordered set of "is this one shown at all", set by
 * `reorderWatchlistGroups`/the owning list's own order, not a second
 * ordering of its own. An id in `shownIds` that no longer names anything in
 * `owned` — a note deleted from one workspace but still named in another
 * workspace's stored settings — is dropped rather than rendered as a blank
 * row: nothing in `owned` describes it any more, so there is nothing left to
 * show.
 */
export function resolveShown<T extends { id: string }>(owned: T[], shownIds: string[]): T[] {
	const shown = new Set(shownIds);
	return owned.filter((item) => shown.has(item.id));
}
