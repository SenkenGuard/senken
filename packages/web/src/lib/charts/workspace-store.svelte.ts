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
import { LAYOUTS, type LayoutId } from '$lib/components/terminal/chart-config';
import { clearLayerStyle } from './layer-style';
import { defaultChartSettings, type ChartSettings } from '$lib/mock/chart-settings';
import {
	defaultWorkspaceSettings,
	parseWorkspaceSettings,
	workspaceSettingsToJson,
	type ChartWorkspaceSettings
} from './workspace-settings';
import {
	paneFromDto,
	drawingToInput,
	layerToInput,
	swapById,
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
// field for "this account's last-selected workspace", so this follows the
// same convention `$lib/api/servers.svelte.ts`
// already uses for "which server is active" — `localStorage`, guarded
// against being unavailable, never treated as a source of truth for
// anything the server itself owns.
const ACTIVE_WORKSPACE_KEY = 'senken.charts.activeWorkspaceId';

export interface IndicatorCatalogItem {
	name: string;
	defaultParams: Record<string, number>;
	placement: 'overlay' | 'sub_pane' | 'either';
}

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
	/** The active workspace's own `settings` text, already parsed — which
	 * watchlist groups and notes its WATCHLIST/NOTES panels show. Kept in
	 * step with `activeWorkspaceId`/`workspaces` by `syncActiveWorkspaceSettings`
	 * rather than re-parsed by every reader, the same reason `layout` itself
	 * is held pre-shaped instead of derived from a raw DTO on each read. */
	workspaceSettings = $state<ChartWorkspaceSettings>(defaultWorkspaceSettings());
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

const parameterTimers = new Map<string, ReturnType<typeof setTimeout>>();

function presetOf(raw: string): LayoutId {
	return (raw in LAYOUTS ? raw : '1') as LayoutId;
}

async function refreshWorkspaceList(): Promise<void> {
	const page = await apiClient.listWorkspaces(200, 0);
	chartWorkspaceStore.workspaces = page.rows;
}

/** Re-derives `workspaceSettings` from whichever `WorkspaceDto` in
 * `workspaces` matches the current `activeWorkspaceId` — called every time
 * either one changes, so a reader of `workspaceSettings` never has to guess
 * whether it still matches the workspace actually on screen. No workspace
 * resolves (nothing loaded yet, or the id was just cleared) falls back to
 * `defaultWorkspaceSettings()`, same as an unconfigured workspace's own
 * `"{}"` would. */
function syncActiveWorkspaceSettings(): void {
	const id = chartWorkspaceStore.activeWorkspaceId;
	const workspace = id ? chartWorkspaceStore.workspaces.find((w) => w.id === id) : undefined;
	chartWorkspaceStore.workspaceSettings = workspace
		? parseWorkspaceSettings(workspace.settings)
		: defaultWorkspaceSettings();
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
		syncActiveWorkspaceSettings();
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
			syncActiveWorkspaceSettings();
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
			syncActiveWorkspaceSettings();
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

async function persistLayer(layer: LayerRuntime): Promise<void> {
	try {
		await apiClient.updateLayer(layer.id, layerToInput(layer));
	} catch (error) {
		chartWorkspaceStore.error = layoutMutationErrorMessage(error);
	}
}

async function persistDrawing(drawing: DrawingRuntime): Promise<void> {
	try {
		await apiClient.updateDrawing(drawing.id, drawingToInput(drawing));
	} catch (error) {
		chartWorkspaceStore.error = layoutMutationErrorMessage(error);
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

/** Changes one pane's main instrument, persisted per user. */
export async function setPaneInstrument(paneIndex: number, instrument: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) => (i === paneIndex ? { ...p, instrument } : p));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}

/** Changes every pane's timeframe at once — the toolbar's timeframe strip
 * applies to the whole layout, matching the former mock UI's `pickTf`. Used
 * when the layout menu's INTERVAL sync toggle is on. */
export async function setAllPanesTimeframe(timeframe: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout) return;
	chartWorkspaceStore.layout = { ...layout, panes: layout.panes.map((p) => ({ ...p, timeframe })) };
	await persistAndReload();
}

/** Changes one pane's timeframe only — the layout menu's INTERVAL sync
 * toggle, off. Mirrors `setPaneInstrument`'s single-pane shape. */
export async function setPaneTimeframe(paneIndex: number, timeframe: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) => (i === paneIndex ? { ...p, timeframe } : p));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}

/** Changes every pane's main instrument at once — the layout menu's SYMBOL
 * sync toggle, on. Mirrors `setAllPanesTimeframe`'s all-panes shape. */
export async function setAllPanesInstrument(instrument: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout) return;
	chartWorkspaceStore.layout = { ...layout, panes: layout.panes.map((p) => ({ ...p, instrument })) };
	await persistAndReload();
}

export async function toggleLayerVisible(paneIndex: number, layerId: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) =>
		i !== paneIndex ? p : { ...p, layers: p.layers.map((l) => (l.id === layerId ? { ...l, visible: !l.visible } : l)) }
	);
	chartWorkspaceStore.layout = { ...layout, panes };
	const layer = panes[paneIndex]?.layers.find((candidate) => candidate.id === layerId);
	if (layer) void persistLayer(layer);
}

/** Swaps two named layers — `aId` and `bId` — in this pane's stacking
 * order, renumbering every layer's `position` to match. Takes the two ids
 * directly rather than a layer plus a direction: the OBJECT TREE panel
 * groups a pane's layers into INSTRUMENTS/INDICATORS, and "move up"/"move
 * down" in the panel must swap within that group, never across it — the
 * panel is the one that knows its own group's ordered ids
 * (`neighborInGroup` in `pane-runtime.ts`), so it names both ids and this
 * function just swaps them in the pane's whole `layers` array, wherever
 * they happen to sit. This has to go through `persistAndReload` rather
 * than `persistLayer`: `crates/chart/src/store.rs`'s `update_pane_item`
 * documents that it ignores `position` entirely (only
 * `replace_layout`'s structural rewrite can move an item), so the
 * cheap per-item PATCH `persistLayer` uses would silently drop the reorder. */
export async function swapLayerOrder(paneIndex: number, aId: string, bId: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const reordered = swapById(layout.panes[paneIndex].layers, aId, bId);
	const panes = layout.panes.map((p, i) => (i !== paneIndex ? p : { ...p, layers: reordered }));
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
	const layer = panes[paneIndex]?.layers.find((candidate) => candidate.id === layerId);
	if (!layer) return;
	const pending = parameterTimers.get(layer.id);
	if (pending) clearTimeout(pending);
	parameterTimers.set(layer.id, setTimeout(() => {
		parameterTimers.delete(layer.id);
		void persistLayer(layer);
	}, 200));
}

/** Writes the layout back so a layer's plot styling — which lives in
 * `layer-style.ts`'s session map and is serialised out by `layerToInput` —
 * is stored. The styling itself is already applied to the chart by the time
 * this runs; this only makes it outlive the page. */
export async function persistLayerStyle(layerId: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout) return;
	const layer = layout.panes.flatMap((pane) => pane.layers).find((candidate) => candidate.id === layerId);
	if (layer) await persistLayer(layer);
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

/** Adds an instrument overlay or indicator layer to `paneIndex`, placed as
 * an overlay or in its own sub-pane. */
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
		kind: item.placement === 'sub_pane' ? 'indicator_sub_pane' : 'indicator_overlay',
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
	try {
		await apiClient.deleteLayer(layerId);
	} catch (error) {
		chartWorkspaceStore.error = layoutMutationErrorMessage(error);
	}
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
	const drawing = panes[paneIndex]?.drawings.find((candidate) => candidate.id === drawingId);
	if (drawing) await persistDrawing(drawing);
}

/** Edits a `text_note` drawing's label — the one property a note has besides
 * the style `updateDrawingStyle` already covers. Persisted the same cheap
 * per-item way: `text` is a field `update_pane_item` actually applies, same
 * as `updateDrawingStyle`'s color/width/line-style. */
export async function updateDrawingText(paneIndex: number, drawingId: string, text: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) =>
		i !== paneIndex ? p : { ...p, drawings: p.drawings.map((d) => (d.id === drawingId ? { ...d, text } : d)) }
	);
	chartWorkspaceStore.layout = { ...layout, panes };
	const drawing = panes[paneIndex]?.drawings.find((candidate) => candidate.id === drawingId);
	if (drawing) await persistDrawing(drawing);
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
	const drawing = panes[paneIndex]?.drawings.find((candidate) => candidate.id === drawingId);
	if (drawing) await persistDrawing(drawing);
}

/** Toggles a drawing's `visible` flag — the object tree's eye/eye-off
 * button, and the floating `DrawingToolbar`'s own show/hide toggle for
 * whichever drawing is selected. Mirrors `toggleLayerVisible` exactly, on
 * `DrawingDto.visible` instead of a layer's, and persists through
 * `persistDrawing`: a per-item PATCH is enough here because `visible`
 * (unlike `position`, see `swapDrawingOrder` below) is a field
 * `update_pane_item` actually applies. */
export async function toggleDrawingVisible(paneIndex: number, drawingId: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) =>
		i !== paneIndex ? p : { ...p, drawings: p.drawings.map((d) => (d.id === drawingId ? { ...d, visible: !d.visible } : d)) }
	);
	chartWorkspaceStore.layout = { ...layout, panes };
	const drawing = panes[paneIndex]?.drawings.find((candidate) => candidate.id === drawingId);
	if (drawing) void persistDrawing(drawing);
}

/** Swaps two named drawings — `aId` and `bId` — in this pane's stacking
 * order, the same way `swapLayerOrder` does for a layer (see its doc for
 * why this takes two ids rather than a direction), and, like
 * `swapLayerOrder`, through `persistAndReload` rather than `persistDrawing`,
 * since a `position` change does not survive `update_pane_item`'s per-item
 * PATCH (`crates/chart/src/store.rs`'s own doc on that method). */
export async function swapDrawingOrder(paneIndex: number, aId: string, bId: string): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const reordered = swapById(layout.panes[paneIndex].drawings, aId, bId);
	const panes = layout.panes.map((p, i) => (i !== paneIndex ? p : { ...p, drawings: reordered }));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
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
	try {
		await apiClient.deleteDrawing(drawingId);
	} catch (error) {
		chartWorkspaceStore.error = layoutMutationErrorMessage(error);
	}
}

/** Patches the active workspace's display settings — which watchlist groups
 * and notes its WATCHLIST/NOTES panels show — merging `patch` onto the
 * in-memory copy the same way `updatePaneSettings` merges a chart-settings
 * patch. Unlike a pane's settings, this has no `replaceLayout` to ride
 * along with: it writes straight through `updateWorkspaceSettings`, and
 * `workspaces` (the one cache of the workspace list) is patched in place so
 * a later `syncActiveWorkspaceSettings` — from a rename, a delete elsewhere,
 * or a fresh `refreshWorkspaceList` — reads the same text back rather than
 * a stale copy from before this write. */
export async function updateWorkspaceSettings(patch: Partial<ChartWorkspaceSettings>): Promise<void> {
	const workspaceId = chartWorkspaceStore.activeWorkspaceId;
	if (!workspaceId) return;
	const next: ChartWorkspaceSettings = { ...chartWorkspaceStore.workspaceSettings, ...patch };
	const nextJson = workspaceSettingsToJson(next);
	chartWorkspaceStore.error = null;
	try {
		await apiClient.updateWorkspaceSettings(workspaceId, nextJson);
		chartWorkspaceStore.workspaceSettings = next;
		chartWorkspaceStore.workspaces = chartWorkspaceStore.workspaces.map((w) =>
			w.id === workspaceId ? { ...w, settings: nextJson } : w
		);
	} catch (error) {
		chartWorkspaceStore.error = layoutMutationErrorMessage(error);
	}
}

/** Clears every drawing in `paneIndex` only — the draw toolbar's eraser
 * button sits in the per-chart drawing rail, and a click on one pane's rail
 * must not erase the other panes in the same layout. */
export async function clearPaneDrawings(paneIndex: number): Promise<void> {
	const layout = chartWorkspaceStore.layout;
	if (!layout || !layout.panes[paneIndex]) return;
	const panes = layout.panes.map((p, i) => (i !== paneIndex ? p : { ...p, drawings: [] }));
	chartWorkspaceStore.layout = { ...layout, panes };
	await persistAndReload();
}
