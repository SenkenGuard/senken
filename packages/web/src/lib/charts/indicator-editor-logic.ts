// Pure decision logic for the indicator-authoring dock
// (`indicator-editor.svelte.ts`), kept free of runes and of `apiClient` the
// same way `indicator-panel.ts` split "what should happen" from "the store
// that makes it happen" for the authoring modal this dock replaces — so the
// property that actually matters (dirty tracking never lies, switching
// indicators never silently discards an edit) is provable under plain
// `bun test`, without a Svelte compile step or a mounted component.
import type { IndicatorCatalogEntry, UserIndicatorDiagnosticDto } from '$lib/api/types';
import type { IndicatorCatalogItem } from './workspace-store.svelte';

export interface IndicatorSnapshot {
	title: string;
	source: string;
}

/** Whether the open indicator's draft differs from what was last saved.
 * `saved === null` means nothing has ever been saved for the id currently
 * open (a brand-new, not-yet-created draft) — dirty exactly when there is
 * anything typed at all, since there is nothing on the server yet to match. */
export function computeDirty(saved: IndicatorSnapshot | null, draft: IndicatorSnapshot): boolean {
	if (saved === null) return draft.title.trim().length > 0 || draft.source.trim().length > 0;
	return saved.title !== draft.title || saved.source !== draft.source;
}

export type SelectDecision = { kind: 'prompt'; targetId: string | null } | { kind: 'proceed'; targetId: string | null };

/** What `requestSelect` should do: ask first only when there are unsaved
 * changes on a *different* indicator than the one being requested — asking
 * again to reopen the very indicator that is already open (or to select
 * `null`/close while genuinely nothing has changed) would be a confirmation
 * dialog with nothing to confirm. */
export function decideSelect(dirty: boolean, currentId: string | null, requestedId: string | null): SelectDecision {
	if (dirty && currentId !== requestedId) return { kind: 'prompt', targetId: requestedId };
	return { kind: 'proceed', targetId: requestedId };
}

/** A stored indicator's `compile_error` (a plain, already-final message —
 * see `UserIndicatorSummaryDto`'s own doc on why line/column are not
 * persisted) recast as the same `UserIndicatorDiagnosticDto` shape a fresh
 * compile response carries, so the editor's diagnostics strip has one
 * rendering path for both "just failed to compile" and "opened an
 * indicator that previously failed". */
export function diagnosticsFromCompileError(message: string | null | undefined): UserIndicatorDiagnosticDto[] {
	return message ? [{ line: null, column: null, message }] : [];
}

/** "Add to chart" must never place a version of the indicator the reader
 * has since edited away from, and never a version that has not compiled at
 * all — both cases mean save first. */
export function shouldSaveBeforeAdding(dirty: boolean, currentlyCompiled: boolean): boolean {
	return dirty || !currentlyCompiled;
}

/** Turns a freshly compiled `IndicatorCatalogEntry` (`GET
 * /api/indicators`'s own catalogue shape) into the `IndicatorCatalogItem`
 * `addIndicatorLayer` accepts — the same conversion the built-in indicator
 * picker in `+page.svelte` already does, so placing a user's own indicator
 * goes through the exact same call as placing a built-in one. */
export function catalogItemFromEntry(entry: IndicatorCatalogEntry): IndicatorCatalogItem {
	return {
		name: entry.name,
		defaultParams: Object.fromEntries(entry.params.map((param) => [param.name, param.default.value])),
		placement: entry.placement
	};
}
