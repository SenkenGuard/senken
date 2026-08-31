// Reactive workspace/layout controller for the charts page. Mirrors `$lib/api/session.svelte.ts`'s shape (a small `$state` class,
// exported as one singleton instance) rather than `@tanstack/svelte-query`:
// unlike a plain cached GET (bars, the instrument catalog — its own
// example), this is mutation-heavy owned state with no natural "query key"
// per mutation, so a hand-rolled store fits better here.
//
// Persistence model ("reopening the app returns you to what you left"): `senken_workspace::WorkspaceStore` has no per-pane or
// per-layer patch method — only whole-workspace CRUD and
// `replace_layout(layout_id, preset, panes)` (the implementation
// notes) — so every pane/layer mutation here calls `replaceLayout` with the
// layout's *entire* new shape, then re-fetches the layout so the UI's pane/
// layer ids match whatever the server actually assigned (a `PaneInputDto`/
// `LayerInputDto` carries no id at all — `replace_layout` mints fresh ones
// on every call, per its own doc: "replaces ... in one transaction").
import { apiClient } from '$lib/api/client';
import { getErrorMessage } from '$lib/api/errors';
import { layoutMutationErrorMessage } from './workspace-error';
import type { WorkspaceDto } from '$lib/api/types';
import { LAYOUTS, type IndicatorCatalogItem, type LayoutId } from '$lib/components/terminal/chart-config';
import { clearLayerStyle } from './layer-style';
import { defaultChartSettings, type ChartSettings } from '$lib/mock/chart-settings';
import {
	paneFromDto,
	toReplaceLayoutRequest,
	type DrawingRuntime,
	type LayerRuntime,
	type PaneRuntime
} from './pane-runtime';

// Which workspace this *browser* had open last — not the workspace/layout
// data itself (that is fully server-persisted, the subject),
// but a client-side convenience so "reopen the app" lands back on the tab
// the user left rather than always resetting to `GET
// /api/workspaces/default`'s one fixed answer. `senken_workspace` has no
// field for "this account's last-selected workspace" and B1 does not ask
// for one, so this follows the same convention `$lib/api/servers.svelte.ts`
// already uses for "which server is active" — `localStorage`, guarded
// against being unavailable, never treated as a source of truth for
// anything the server itself owns.
const ACTIVE_WORKSPACE_KEY = 'senken.charts.activeWorkspaceId';

function loadRememberedWorkspaceId(): string | null {
	try {
		return localStorage.getItem(ACTIVE_WORKSPACE_KEY);
	} catch {
		return null;
	}
}

function rememberWorkspaceId(id: string): void {
	try {
		localStorage.setItem(ACTIVE_WORKSPACE_KEY, id);
	} catch {
		// Best-effort, same as `credential-store.ts`.
	}
}

export interface LoadedLayout {
	id: string;
	workspaceId: string;
	name: string;
	preset: LayoutId;
	panes: PaneRuntime[];
}

class ChartWorkspaceStore {
	workspaces = $state<WorkspaceDto[]>([]);
	activeWorkspaceId = $state<string | null>(null);
	layout = $state<LoadedLayout | null>(null);
	/** True only while the very first load (workspace list + default
	 * workspace/layout) is in flight — individual mutations set
	 * `mutating`, not this, so the whole grid doesn't unmount on every
	 * click. */
	loading = $state(true);
	/** Set only while a mutation's own `replaceLayout` round-trip is in
	 * flight, so callers can disable a control without hiding the grid. */
	mutating = $state(false);
	error = $state<string | null>(null);
}

export const chartWorkspaceStore = new ChartWorkspaceStore();

function presetOf(raw: string): LayoutId {
	return (raw in LAYOUTS ? raw : '1') as LayoutId;
}

async function refreshWorkspaceList(): Promise<void> {
	const page = await apiClient.listWorkspaces(200, 0);
	chartWorkspaceStore.workspaces = page.rows;
}

async function firstLayoutId(workspaceId: string): Promise<string> {
	const layouts = await apiClient.listLayouts(workspaceId);
	if (layouts.length === 0) {
		throw new Error(`workspace ${workspaceId} has no layout`);
	}
	return layouts[0].id;
}

async function loadLayoutInto(workspaceId: string, layoutId: string): Promise<void> {
	const detail = await apiClient.getLayout(layoutId);
	chartWorkspaceStore.layout = {
		id: detail.layout.id,
		workspaceId,
		name: detail.layout.name,
		preset: presetOf(detail.layout.preset),
		panes: detail.panes.map(paneFromDto).sort((a, b) => a.position - b.position)
	};
}

/** Switches to `workspaceId`'s given layout, or its first one if
 * `layoutId` is omitted (every workspace the app can create today has
 * exactly one — the implementation notes on why "create more"
 * means more workspaces, not more layouts per workspace). */
export async function selectWorkspace(workspaceId: string, layoutId?: string): Promise<void> {
	chartWorkspaceStore.error = null;
	try {
		const resolvedLayoutId = layoutId ?? (await firstLayoutId(workspaceId));
		await loadLayoutInto(workspaceId, resolvedLayoutId);
		chartWorkspaceStore.activeWorkspaceId = workspaceId;
		rememberWorkspaceId(workspaceId);
	} catch (error) {
		chartWorkspaceStore.error = getErrorMessage(error, 'Could not open that workspace.');
	}
}

/** First load: `GET /api/workspaces/default` (creates the
 * caller's default workspace server-side on the very first call, so it
 * holds for any client), then the workspace list for the switcher, then
 * that default's one layout. */
export async function initChartWorkspaces(): Promise<void> {
	chartWorkspaceStore.loading = true;
	chartWorkspaceStore.error = null;
	try {
		// `GET /api/workspaces/default` always names the same one fixed
		// workspace (the "default-on-first-open"); it is only the
		// *fallback* here, not the answer, so a browser that remembers a
		// different workspace it had open last (see `rememberWorkspaceId`'s
		// doc above) reopens there instead — matching the "reopening
		// the app returns you to what you left" for the tab itself, not only
		// for the data inside it.
		const [def] = await Promise.all([apiClient.defaultWorkspace(), refreshWorkspaceList()]);
		const remembered = loadRememberedWorkspaceId();
		const target =
			remembered && chartWorkspaceStore.workspaces.some((w) => w.id === remembered) ? remembered : def.workspace_id;
		if (target === def.workspace_id) {
			await loadLayoutInto(def.workspace_id, def.layout_id);
			chartWorkspaceStore.activeWorkspaceId = def.workspace_id;
		} else {
			await selectWorkspace(target);
		}
		rememberWorkspaceId(chartWorkspaceStore.activeWorkspaceId ?? def.workspace_id);
	} catch (error) {
		chartWorkspaceStore.error = getErrorMessage(error, 'Could not load your workspaces.');
	} finally {
		chartWorkspaceStore.loading = false;
	}
}

export async function createNewWorkspace(name: string): Promise<void> {
	chartWorkspaceStore.mutating = true;
	chartWorkspaceStore.error = null;
	try {
		const { id } = await apiClient.createWorkspace(name);
		await refreshWorkspaceList();
		await selectWorkspace(id);
	} catch (error) {
		chartWorkspaceStore.error = getErrorMessage(error, 'Could not create that workspace.');
	} finally {
		chartWorkspaceStore.mutating = false;
	}
}

export async function renameWorkspace(workspaceId: string, name: string): Promise<void> {
	chartWorkspaceStore.error = null;
	try {
		await apiClient.renameWorkspace(workspaceId, name);
		await refreshWorkspaceList();
	} catch (error) {
		chartWorkspaceStore.error = getErrorMessage(error, 'Could not rename that workspace.');
	}
}

/** Deletes `workspaceId`. If it was the active one, falls back to the
 * server's default workspace (never leaves the page pointed at a workspace
 * that no longer exists). */
export async function deleteWorkspace(workspaceId: string): Promise<void> {
	chartWorkspaceStore.mutating = true;
	chartWorkspaceStore.error = null;
	try {
		const wasActive = chartWorkspaceStore.activeWorkspaceId === workspaceId;
		await apiClient.deleteWorkspace(workspaceId);
		await refreshWorkspaceList();
		if (wasActive) {
			const def = await apiClient.defaultWorkspace();
			await loadLayoutInto(def.workspace_id, def.layout_id);
			chartWorkspaceStore.activeWorkspaceId = def.workspace_id;
			rememberWorkspaceId(def.workspace_id);
		}
	} catch (error) {
		chartWorkspaceStore.error = getErrorMessage(error, 'Could not delete that workspace.');
	} finally {
		chartWorkspaceStore.mutating = false;
	}
}

/** Persists the layout's current in-memory `preset`/`panes`, then re-reads
 * it back so pane/layer ids match what the server actually assigned. Every
 * mutation below funnels through this one function.
 *
 * A rejected save re-reads the layout too, on the same `try`, so memory
 * matches what the server actually holds — the pre-save code left the
 * optimistic (rejected) edit sitting in `chartWorkspaceStore.layout`
 * indefinitely, silently discarded only whenever some *later*, unrelated
 * mutation happened to succeed and overwrite it. A 403 gets its own
 * message: this is an authorization outcome ("you may not edit this
 * layout"), not a generic failure, and — like every other 403 in this
 * app — it must never be treated as a dead session. */
/** Saves the layout without re-reading it.
 *
 * The re-read in `persistAndReload` exists to pick up ids the server
 * assigns, and for a structural change — a pane added, a layer created —
 * that is exactly right. For an *edit* to something that already exists
 * (an indicator's period, a plot colour) nothing server-assigned changes,
 * and re-reading replaces every pane object in the store: the chart
 * re-frames, the open dialog remounts, and a user turning a period from 50
 * to 14 watches their view reset under them on each keystroke.
 *
 * A failure here still surfaces and still re-reads, because then the memory
 * genuinely does disagree with the server.
 */
async function persistOnly(): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout) return;
	chartWorkspaceStore.error = null;
	try {
		await apiClient.replaceLayout(layout.id, toReplaceLayoutRequest(layout.preset, layout.panes));
	} catch (error) {
		chartWorkspaceStore.error = layoutMutationErrorMessage(error);
		try {
			await loadLayoutInto(layout.workspaceId, layout.id);
		} catch {
			// Unreachable server: keep the stale copy rather than losing the
			// chart on top of the failed save.
		}
	}
}

async function persistAndReload(): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout) return;
	chartWorkspaceStore.mutating = true;
	chartWorkspaceStore.error = null;
	try {
		await apiClient.replaceLayout(layout.id, toReplaceLayoutRequest(layout.preset, layout.panes));
		await loadLayoutInto(layout.workspaceId, layout.id);
	} catch (error) {
		chartWorkspaceStore.error = layoutMutationErrorMessage(error);
		try {
			await loadLayoutInto(layout.workspaceId, layout.id);
		} catch {
			// The layout can't be re-read right now either (the server is
			// unreachable, say) — leave the stale in-memory copy rather than
			// wiping it to `null` and losing the chart entirely on top of
			// the save failure.
		}
	} finally {
		chartWorkspaceStore.mutating = false;
	}
}

/** Clears a save failure once the user has seen it — the charts route's
 * dismissible inline error banner calls this. */
export function dismissWorkspaceError(): void {
	chartWorkspaceStore.error = null;
}

/** `crates/workspace/src/store.rs`'s own `default_instrument()` — the exact
 * fallback the server itself seeds a brand new workspace with, reused here
 * (not guessed at) for the one defensive case a layout has no pane at all
 * to inherit from. */
const FALLBACK_INSTRUMENT = 'binance-spot:BTCUSDT';

/** Mirrors the former mock's `setLayout`: existing panes are kept as-is up
 * to the new preset's pane count. New ones inherit the first pane's
 * instrument at the current timeframe — this used to cycle through
 * `searchInstruments('', 8)`, an unrestricted catalog search that could
 * (and did, with `whitebit:ABTC`) hand a new pane an instrument from one
 * of the ~47 registered sources with no bar source at all, which cannot
 * chart. The first pane's own instrument is always chartable, since it is
 * already on screen; a distinct default needs the server to say which
 * sources can serve bars, which does not exist yet. */
export async function setLayoutPreset(preset: LayoutId): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout) return;
	const need = LAYOUTS[preset].n;
	const timeframe = layout.panes[0]?.timeframe ?? '1h';
	const inheritedInstrument = layout.panes[0]?.instrument ?? FALLBACK_INSTRUMENT;
	const kept = layout.panes.slice(0, need);
	const panes: PaneRuntime[] = [...kept];
	while (panes.length < need) {
		panes.push({
			id: `pending-${panes.length}`,
			position: panes.length,
			instrument: inheritedInstrument,
			timeframe,
			layers: [],
			drawings: [],
			settings: defaultChartSettings()
		});
	}
	chartWorkspaceStore.layout = { ...layout, preset, panes: panes.map((p, i) => ({ ...p, position: i })) };
	await persistAndReload();
}

/** Changes one pane's main instrument (the acceptance point 5:
 * "change instrument ... persisted per user"). */
export async function setPaneInstrument(paneIndex: number, instrument: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) => (i === paneIndex ? { ...p, instrument } : p));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}

/** Changes every pane's timeframe at once — the toolbar's timeframe strip
 * applies to the whole layout, matching the former mock UI's `pickTf`. */
export async function setAllPanesTimeframe(timeframe: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout) return;
	chartWorkspaceStore.layout = { ...layout, panes: layout.panes.map((p) => ({ ...p, timeframe })) };
	await persistAndReload();
}

export async function toggleLayerVisible(paneIndex: number, layerId: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) =>
		i !== paneIndex ? p : { ...p, layers: p.layers.map((l) => (l.id === layerId ? { ...l, visible: !l.visible } : l)) }
	);
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}

/** Patches an indicator layer's construction parameters (period, fast/
 * slow/signal, k, kPeriod/dPeriod) — these persist for real, inside the
 * layer's `params` JSON (`$lib/charts/pane-runtime.ts`). Style edits do
 * not go through here; see `$lib/charts/layer-style.ts`. */
export async function editLayerParams(paneIndex: number, layerId: string, patch: Record<string, number>): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) =>
		i !== paneIndex ? p : { ...p, layers: p.layers.map((l) => (l.id === layerId ? { ...l, params: { ...l.params, ...patch } } : l)) }
	);
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistOnly();
}

/** Writes the layout back so a layer's plot styling — which lives in
 * `layer-style.ts`'s session map and is serialised out by `layerToInput` —
 * is stored. The styling itself is already applied to the chart by the time
 * this runs; this only makes it outlive the page. */
export async function persistLayerStyle(): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout) return;
	// Re-assigning the same panes is what marks the layout dirty: the style
	// lives beside the runtime objects rather than inside them, so nothing
	// here changes shape.
	chartWorkspaceStore.layout = { ...layout, panes: [...layout.panes] };
	await persistOnly();
}

/** Patches `paneIndex`'s chart settings. The settings dialog used to hold
 * this in a single page-level `$state` object shared by every pane, with
 * nowhere on `PaneDto` to persist it, so a reload always lost it — each
 * pane now carries and persists its own. `patch` is merged onto the pane's
 * existing settings the same way `editLayerParams` merges an indicator's
 * params, so a caller only ever names the fields that changed. */
export async function updatePaneSettings(paneIndex: number, patch: Partial<ChartSettings>): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) => (i !== paneIndex ? p : { ...p, settings: { ...p.settings, ...patch } }));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistOnly();
}

/** Adds an instrument overlay or indicator layer to `paneIndex` (the acceptance point 6: add, overlay/sub-pane). */
export async function addInstrumentOverlay(paneIndex: number, instrument: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const pane = layout.panes[paneIndex];
	const layer: LayerRuntime = { id: `pending-${pane.layers.length}`, position: pane.layers.length, kind: 'overlay_instrument', visible: true, instrument, params: {} };
	const panes = layout.panes.map((p, i) => (i !== paneIndex ? p : { ...p, layers: [...p.layers, layer] }));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}

export async function addIndicatorLayer(paneIndex: number, item: IndicatorCatalogItem): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const pane = layout.panes[paneIndex];
	const layer: LayerRuntime = {
		id: `pending-${pane.layers.length}`,
		position: pane.layers.length,
		kind: item.subPane ? 'indicator_sub_pane' : 'indicator_overlay',
		visible: true,
		indicatorName: item.name,
		params: { ...item.defaultParams }
	};
	const panes = layout.panes.map((p, i) => (i !== paneIndex ? p : { ...p, layers: [...p.layers, layer] }));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}

/** Removes a layer. The pane's main instrument is never a layer in this
 * model (see `pane-runtime.ts`'s header comment), so there is no "locked
 * primary layer" case to guard here — every layer this function can see is
 * removable. */
export async function removeLayer(paneIndex: number, layerId: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	clearLayerStyle(layerId);
	const panes = layout.panes.map((p, i) =>
		i !== paneIndex ? p : { ...p, layers: p.layers.filter((l) => l.id !== layerId).map((l, idx) => ({ ...l, position: idx })) }
	);
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}

/** Adds a drawing object — a horizontal line, trend line or rectangle — to
 * `paneIndex` (create is the first of "create, hit-test on click, select, edit properties, delete, persist"). `id`/`position` are
 * assigned here the same way every other "pending" runtime object in this
 * module is (see `addInstrumentOverlay`'s identical pattern) — the real id
 * comes back once `persistAndReload` re-fetches the layout. */
export async function addDrawing(paneIndex: number, drawing: Omit<DrawingRuntime, 'id' | 'position'>): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const pane = layout.panes[paneIndex];
	const withIds: DrawingRuntime = { ...drawing, id: `pending-${pane.drawings.length}`, position: pane.drawings.length };
	const panes = layout.panes.map((p, i) => (i !== paneIndex ? p : { ...p, drawings: [...p.drawings, withIds] }));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}

/** Edits a drawing's colour/width/line-style — the three properties a
 * TradingView-style "edit object" panel offers. */
export async function updateDrawingStyle(
	paneIndex: number,
	drawingId: string,
	patch: Partial<Pick<DrawingRuntime, 'color' | 'width' | 'lineStyle'>>
): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) =>
		i !== paneIndex ? p : { ...p, drawings: p.drawings.map((d) => (d.id === drawingId ? { ...d, ...patch } : d)) }
	);
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistOnly();
}

/** Moves a drawing: a new anchor price for a horizontal line, or new
 * endpoints for a trend line or rectangle. Separate from
 * `updateDrawingStyle` because the two are edited by different gestures —
 * dragging a handle versus picking a colour — even though both end in the
 * same write. */
export async function updateDrawingGeometry(
	paneIndex: number,
	drawingId: string,
	patch: Partial<Pick<DrawingRuntime, 'price' | 'start' | 'end'>>
): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) =>
		i !== paneIndex
			? p
			: { ...p, drawings: p.drawings.map((d) => (d.id === drawingId ? { ...d, ...patch } : d)) }
	);
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistOnly();
}

/** Deletes one drawing. */
export async function removeDrawing(paneIndex: number, drawingId: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) =>
		i !== paneIndex
			? p
			: { ...p, drawings: p.drawings.filter((d) => d.id !== drawingId).map((d, idx) => ({ ...d, position: idx })) }
	);
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}

/** Clears every drawing in every pane of the active layout — the draw
 * toolbar's eraser button, matching the reference's own `clearDrawings`
 * (which tears down and rebuilds every chart in the workspace, not just the
 * active pane's). */
export async function clearAllDrawings(): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout) return;
	const panes = layout.panes.map((p) => ({ ...p, drawings: [] }));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}
