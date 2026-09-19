// Reactive controller for the indicator-authoring dock (`030`'s "panel
// khusus" — TradingView's Pine Editor is the shape CEO pointed at). Mirrors
// `workspace-store.svelte.ts`'s own shape (a small `$state` class, exported
// as one singleton) for the same reason that file gives for itself:
// mutation-heavy owned state with no natural cache key, so a hand-rolled
// store fits better than a query cache.
//
// Every network call goes through `apiClient.myIndicators.*`
// (`crates/api/src/user_indicator_handlers.rs`, landed by 033) — this
// module owns no fetch logic of its own beyond calling that surface and
// reacting to what it returns.
import { apiClient } from '$lib/api/client';
import { getErrorMessage } from '$lib/api/errors';
import { addIndicatorLayer } from './workspace-store.svelte';
import { refreshIndicatorCatalog } from './indicator-catalog.svelte';
import {
	computeDirty,
	decideSelect,
	diagnosticsFromCompileError,
	shouldSaveBeforeAdding,
	catalogItemFromEntry,
	type IndicatorSnapshot
} from './indicator-editor-logic';
import type { IndicatorToolchainStatusResponse, UserIndicatorDiagnosticDto, UserIndicatorSummaryDto } from '$lib/api/types';
import { INDICATOR_TEMPLATES, type IndicatorTemplateId } from '$lib/components/indicators/templates';

const DOCK_OPEN_KEY = 'senken.charts.indicatorDockOpen';

function loadRememberedOpen(): boolean {
	try {
		return localStorage.getItem(DOCK_OPEN_KEY) === '1';
	} catch {
		return false;
	}
}

function rememberOpen(open: boolean): void {
	try {
		localStorage.setItem(DOCK_OPEN_KEY, open ? '1' : '0');
	} catch {
		// Best-effort, same as `workspace-store.svelte.ts`'s own
		// `rememberWorkspaceId` — losing this preference is not worth
		// surfacing an error over.
	}
}

class IndicatorEditorStore {
	open = $state(loadRememberedOpen());
	list = $state<UserIndicatorSummaryDto[]>([]);
	listLoading = $state(false);
	activeId = $state<string | null>(null);
	draft = $state<IndicatorSnapshot>({ title: '', source: '' });
	/** The last saved `{title, source}` for `activeId` — dirty is derived by
	 * comparing `draft` against this, never by a separate boolean flag a
	 * forgotten write path could leave stale. */
	saved = $state<IndicatorSnapshot | null>(null);
	saving = $state(false);
	diagnostics = $state<UserIndicatorDiagnosticDto[]>([]);
	toolchain = $state<IndicatorToolchainStatusResponse | null>(null);
	/** Product-facing message for a request that failed outright (network,
	 * 5xx) — distinct from `diagnostics`, which is the author's own Rust
	 * being rejected, not a transport failure. */
	error = $state<string | null>(null);
	/** Set while a `select`/`createNew` targeting a different indicator is
	 * waiting on the reader to confirm discarding unsaved changes. `null`
	 * means no such prompt is open. */
	pendingSelectId = $state<string | null | undefined>(undefined);

	dirty = $derived(computeDirty(this.saved, this.draft));
}

export const indicatorEditor = new IndicatorEditorStore();

/** Fetches the toolchain's availability once — called when the dock first
 * mounts, so Save can be disabled with a reason before the reader has typed
 * anything a failed compile would otherwise blame on them. */
export async function loadToolchainStatus(): Promise<void> {
	try {
		indicatorEditor.toolchain = await apiClient.myIndicators.toolchain();
	} catch (err) {
		// Treated as "unknown", not "unavailable" — the banner only ever
		// states a reason the server itself gave.
		indicatorEditor.toolchain = null;
		indicatorEditor.error = getErrorMessage(err, 'Could not reach the server.');
	}
}

/** (Re)loads the list of the signed-in account's own indicators. Called on
 * mount and after every mutation (create/save/rename/delete) so the list's
 * compiled/error dot never lags behind what the server actually holds. */
export async function loadIndicatorList(): Promise<void> {
	indicatorEditor.listLoading = true;
	try {
		indicatorEditor.list = await apiClient.myIndicators.list();
		indicatorEditor.error = null;
	} catch (err) {
		indicatorEditor.error = getErrorMessage(err, 'Could not load your indicators.');
	} finally {
		indicatorEditor.listLoading = false;
	}
}

/** Loads one indicator's full source into the editor, unconditionally —
 * callers that need the dirty guard call `requestSelect` instead, which
 * decides whether to ask first. */
async function reallySelect(id: string): Promise<void> {
	try {
		const row = await apiClient.myIndicators.get(id);
		indicatorEditor.activeId = row.id;
		indicatorEditor.draft = { title: row.title, source: row.source };
		indicatorEditor.saved = { title: row.title, source: row.source };
		indicatorEditor.diagnostics = diagnosticsFromCompileError(row.compile_error);
		indicatorEditor.error = null;
	} catch (err) {
		indicatorEditor.error = getErrorMessage(err, 'Could not load that indicator.');
	}
}

/** Opens `id` in the editor (or clears the editor for `null`), asking first
 * — through `pendingSelectId`, which the dock renders a confirmation dialog
 * for — when the currently open indicator has unsaved changes. */
export function requestSelect(id: string | null): void {
	const decision = decideSelect(indicatorEditor.dirty, indicatorEditor.activeId, id);
	if (decision.kind === 'prompt') {
		indicatorEditor.pendingSelectId = decision.targetId;
		return;
	}
	if (decision.targetId === null) {
		indicatorEditor.activeId = null;
		indicatorEditor.draft = { title: '', source: '' };
		indicatorEditor.saved = null;
		indicatorEditor.diagnostics = [];
		return;
	}
	void reallySelect(decision.targetId);
}

/** The reader confirmed "Discard changes?" — proceeds with whatever
 * `requestSelect` call raised the prompt. */
export function confirmDiscardAndSelect(): void {
	const target = indicatorEditor.pendingSelectId;
	indicatorEditor.pendingSelectId = undefined;
	if (target === undefined) return;
	if (target === null) {
		indicatorEditor.activeId = null;
		indicatorEditor.draft = { title: '', source: '' };
		indicatorEditor.saved = null;
		indicatorEditor.diagnostics = [];
		return;
	}
	void reallySelect(target);
}

export function cancelDiscard(): void {
	indicatorEditor.pendingSelectId = undefined;
}

export function setDraftSource(source: string): void {
	indicatorEditor.draft = { ...indicatorEditor.draft, source };
}

export function setDraftTitle(title: string): void {
	indicatorEditor.draft = { ...indicatorEditor.draft, title };
}

/** Creates a new indicator from a template and opens it, compiling
 * immediately (the endpoint always does). Returns whether that first compile
 * succeeded, so the New dialog can close either way while the dock itself
 * shows any diagnostics. */
export async function createIndicator(title: string, template: IndicatorTemplateId): Promise<boolean> {
	const source = INDICATOR_TEMPLATES.find((t) => t.id === template)?.source ?? INDICATOR_TEMPLATES[0].source;
	indicatorEditor.saving = true;
	indicatorEditor.error = null;
	try {
		const response = await apiClient.myIndicators.create({ title, source });
		indicatorEditor.activeId = response.id;
		indicatorEditor.draft = { title, source };
		indicatorEditor.saved = { title, source };
		indicatorEditor.diagnostics = response.diagnostics ?? [];
		await loadIndicatorList();
		refreshIndicatorCatalog();
		return response.compiled;
	} catch (err) {
		indicatorEditor.error = getErrorMessage(err, 'Could not create that indicator.');
		return false;
	} finally {
		indicatorEditor.saving = false;
	}
}

/** Saves the currently open indicator's draft and compiles it. A failed
 * compile still returns normally (the server's own `200` — see
 * `SaveUserIndicatorResponse`'s doc): the previous compiled component is
 * left in place, `diagnostics` is populated, and `dirty` is cleared,
 * because the *save* succeeded even though the *compile* did not — editing
 * again is what should re-arm the "unsaved changes" guard, not a compile
 * error the reader has already seen. A genuine transport failure (network,
 * 5xx) leaves `dirty` alone instead, so the edit is never silently lost. */
export async function saveIndicator(): Promise<boolean> {
	const id = indicatorEditor.activeId;
	if (!id) return false;
	indicatorEditor.saving = true;
	indicatorEditor.error = null;
	try {
		const response = await apiClient.myIndicators.update(id, {
			source: indicatorEditor.draft.source,
			title: indicatorEditor.draft.title
		});
		indicatorEditor.saved = { ...indicatorEditor.draft };
		indicatorEditor.diagnostics = response.diagnostics ?? [];
		await loadIndicatorList();
		refreshIndicatorCatalog();
		return response.compiled;
	} catch (err) {
		indicatorEditor.error = getErrorMessage(err, 'Could not save that indicator.');
		return false;
	} finally {
		indicatorEditor.saving = false;
	}
}

/** Renames the currently open indicator — a save with only the title
 * changed, which still recompiles the unchanged source (the endpoint has
 * no narrower "rename only" form; see `UpdateUserIndicatorRequest`'s own
 * doc on why `title` is optional there but not separated into its own
 * route). */
export async function renameIndicator(id: string, title: string): Promise<void> {
	if (id !== indicatorEditor.activeId) return;
	setDraftTitle(title);
	await saveIndicator();
}

export async function deleteIndicator(id: string): Promise<void> {
	indicatorEditor.error = null;
	try {
		await apiClient.myIndicators.remove(id);
		if (indicatorEditor.activeId === id) {
			indicatorEditor.activeId = null;
			indicatorEditor.draft = { title: '', source: '' };
			indicatorEditor.saved = null;
			indicatorEditor.diagnostics = [];
		}
		await loadIndicatorList();
		refreshIndicatorCatalog();
	} catch (err) {
		indicatorEditor.error = getErrorMessage(err, 'Could not delete that indicator.');
	}
}

/** Places the currently open indicator onto `paneIndex`. Saves first when
 * the draft has unsaved changes or has never compiled — "Add to chart"
 * always shows the reader whatever is on the chart right now, never a
 * version of the source they have since edited away from. Returns a
 * product-facing outcome for the toolbar to show; never throws. */
export async function addActiveIndicatorToChart(paneIndex: number): Promise<{ ok: boolean; message?: string }> {
	const id = indicatorEditor.activeId;
	if (!id) return { ok: false, message: 'Open an indicator first.' };

	const currentlyCompiled = indicatorEditor.list.find((row) => row.id === id)?.compiled ?? false;
	if (shouldSaveBeforeAdding(indicatorEditor.dirty, currentlyCompiled)) {
		const compiled = await saveIndicator();
		if (!compiled) return { ok: false, message: 'Fix the build error before adding this to the chart.' };
	}

	const slug = indicatorEditor.list.find((row) => row.id === id)?.slug;
	if (!slug) return { ok: false, message: 'Could not find the compiled indicator.' };

	try {
		const entries = await apiClient.listIndicators();
		const entry = entries.find((e) => e.name === `my/${slug}`);
		if (!entry) return { ok: false, message: 'The compiled indicator is not in the catalogue yet — try again in a moment.' };
		await addIndicatorLayer(paneIndex, catalogItemFromEntry(entry));
		refreshIndicatorCatalog();
		return { ok: true };
	} catch (err) {
		return { ok: false, message: getErrorMessage(err, 'Could not add this indicator to the chart.') };
	}
}

export function toggleDock(open?: boolean): void {
	indicatorEditor.open = open ?? !indicatorEditor.open;
	rememberOpen(indicatorEditor.open);
}
