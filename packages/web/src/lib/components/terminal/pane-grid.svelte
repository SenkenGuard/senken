<script lang="ts">
	// Renders the 1 / 2h / 2v / 3h / 3v / 4 layout presets (layoutMap,
	//) as nested paneforge splits. The
	// reference itself lays panes out with a static CSS grid (line 445) —
	// nothing in it is drag-resizable at this level — but the // brief calls out `resizable` (paneforge) as "the intended primitive"
	// for this grid, so panes here are real resizable splits instead. The
	// "4" preset nests a vertical PaneGroup inside each half of an outer
	// horizontal one to get an independently resizable 2x2 grid; pane index
	// order still matches the reference's row-major grid fill (0,1 top row,
	// 2,3 bottom row).
	import * as Resizable from '$lib/components/ui/resizable/index.js';
	import PaneCell from './pane-cell.svelte';
	import type { ChartSettings } from '$lib/mock/chart-settings';
	import type { MarketStatus } from '$lib/charts/live-state';
	import type { DrawingRuntime, PaneRuntime } from '$lib/charts/pane-runtime';
	import type { LayoutId, ToolKey } from './chart-config';

	let {
		panes,
		marketStatuses = {},
		reloadToken = 0,
		resetTokens = [],
		onMoveDrawing,
		onPatchSettings,
		onContextMenu,
		layout,
		activeIndex,
		tool,
		replaying,
		replayCut,
		replayPct,
		clearToken,
		selectedDrawing,
		onFocus,
		onToggleLayer,
		onOpenMainSettings,
		onOpenLayerSettings,
		onRemoveLayer,
		onLastClose,
		onCreateDrawing,
		onSelectDrawing,
		onToolConsumed
	}: {
		panes: PaneRuntime[];
		/** Catalogued session state by instrument. Market data remains outside
		 * the persisted chart layout, which only owns user preferences. */
		marketStatuses?: Record<string, MarketStatus>;
		/** Bumping this reloads every pane's bars — the manual way out of a
		 * chart that has ended up somewhere the user cannot explain. */
		reloadToken?: number;
		/** One reset counter per pane — a shared counter would reset every
		 * pane whenever any one of them was asked. */
		resetTokens?: number[];
		onMoveDrawing?: (
			paneIndex: number,
			id: string,
			patch: Partial<Pick<DrawingRuntime, 'price' | 'start' | 'end'>>
		) => void;
		onContextMenu?: (
			paneIndex: number,
			event: { x: number; y: number; region: 'chart' | 'price-scale' | 'time-scale' }
		) => void;
		layout: LayoutId;
		activeIndex: number;
		tool: ToolKey;
		replaying: boolean;
		replayCut: number | null;
		replayPct: string;
		clearToken: number;
		/** Which drawing is currently selected, if any — scoped to one pane
		 * since only one object is selected at a time across the whole
		 * layout. */
		selectedDrawing: { pane: number; id: string } | null;
		onFocus: (index: number) => void;
		onToggleLayer: (paneIndex: number, layerId: string) => void;
		onOpenMainSettings: (paneIndex: number) => void;
		onOpenLayerSettings: (paneIndex: number, layerId: string) => void;
		onRemoveLayer: (paneIndex: number, layerId: string) => void;
		onLastClose: (paneIndex: number, price: number) => void;
		onCreateDrawing: (paneIndex: number, drawing: Omit<DrawingRuntime, 'id' | 'position'>) => void;
		onPatchSettings?: (paneIndex: number, patch: Partial<ChartSettings>) => void;
		onSelectDrawing: (paneIndex: number, id: string | null) => void;
		onToolConsumed: () => void;
	} = $props();
</script>

{#snippet cell(i: number, size: number)}
	<Resizable.Pane defaultSize={size} minSize={15}>
		<PaneCell
			instrument={panes[i].instrument}
			marketStatus={marketStatuses[panes[i].instrument]}
			spec={panes[i].timeframe}
			layers={panes[i].layers}
			drawings={panes[i].drawings}
			settings={panes[i].settings}
			{reloadToken}
			resetToken={resetTokens[i] ?? 0}
			onContextMenu={(e) => onContextMenu?.(i, e)}
			selectedDrawingId={selectedDrawing?.pane === i ? selectedDrawing.id : null}
			showFocusRing={panes.length > 1 && activeIndex === i}
			{tool}
			{replaying}
			{replayCut}
			{replayPct}
			{clearToken}
			onFocus={() => onFocus(i)}
			onToggleLayer={(id) => onToggleLayer(i, id)}
			onOpenMainSettings={() => onOpenMainSettings(i)}
			onOpenLayerSettings={(id) => onOpenLayerSettings(i, id)}
			onRemoveLayer={(id) => onRemoveLayer(i, id)}
			onLastClose={(price) => onLastClose(i, price)}
			onCreateDrawing={(drawing) => onCreateDrawing(i, drawing)}
			onSelectDrawing={(id) => onSelectDrawing(i, id)}
			onMoveDrawing={(id, patch) => onMoveDrawing?.(i, id, patch)}
			onPatchSettings={(patch) => onPatchSettings?.(i, patch)}
			{onToolConsumed}
		/>
	</Resizable.Pane>
{/snippet}

<div class="min-h-0 min-w-0 flex-1">
	{#if layout === '1'}
		<Resizable.PaneGroup direction="horizontal" class="h-full w-full">
			{@render cell(0, 100)}
		</Resizable.PaneGroup>
	{:else if layout === '2h'}
		<Resizable.PaneGroup direction="horizontal" class="h-full w-full">
			{@render cell(0, 50)}
			<Resizable.Handle />
			{@render cell(1, 50)}
		</Resizable.PaneGroup>
	{:else if layout === '2v'}
		<Resizable.PaneGroup direction="vertical" class="h-full w-full">
			{@render cell(0, 50)}
			<Resizable.Handle />
			{@render cell(1, 50)}
		</Resizable.PaneGroup>
	{:else if layout === '3h'}
		<Resizable.PaneGroup direction="horizontal" class="h-full w-full">
			{@render cell(0, 34)}
			<Resizable.Handle />
			{@render cell(1, 33)}
			<Resizable.Handle />
			{@render cell(2, 33)}
		</Resizable.PaneGroup>
	{:else if layout === '3v'}
		<Resizable.PaneGroup direction="vertical" class="h-full w-full">
			{@render cell(0, 34)}
			<Resizable.Handle />
			{@render cell(1, 33)}
			<Resizable.Handle />
			{@render cell(2, 33)}
		</Resizable.PaneGroup>
	{:else if layout === '4'}
		<Resizable.PaneGroup direction="horizontal" class="h-full w-full">
			<Resizable.Pane defaultSize={50} minSize={20}>
				<Resizable.PaneGroup direction="vertical" class="h-full w-full">
					{@render cell(0, 50)}
					<Resizable.Handle />
					{@render cell(2, 50)}
				</Resizable.PaneGroup>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane defaultSize={50} minSize={20}>
				<Resizable.PaneGroup direction="vertical" class="h-full w-full">
					{@render cell(1, 50)}
					<Resizable.Handle />
					{@render cell(3, 50)}
				</Resizable.PaneGroup>
			</Resizable.Pane>
		</Resizable.PaneGroup>
	{/if}
</div>
