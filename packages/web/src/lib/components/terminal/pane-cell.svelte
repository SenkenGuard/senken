<script lang="ts">
	// One grid cell: chart-pane's candlestick canvas with pane-header's layer
	// chips overlaid on top of it — they
	// share this component's relative wrapper exactly as the reference's
	// `data-chart-pane` div and its sibling chip overlay do. Split out from
	// pane-grid.svelte so each cell keeps its own hover/narrow state.
	//
	// `symbol`/`layers: PaneLayer[]` become `instrument`/`spec`
	// (the pane's own main instrument/timeframe) plus `layers: LayerRuntime[]`
	// (real overlay/sub-pane layers only — see `$lib/charts/pane-runtime.ts`).
	// Any layer with `kind === 'indicator_sub_pane'` gets its own resizable
	// strip below the main chart instead of a header chip, via
	// `splitPaneLayers` — the same discriminant the former mock used, ported
	// onto the new shape.
	import * as Resizable from '$lib/components/ui/resizable/index.js';
	import type { IChartApi, UTCTimestamp } from 'lightweight-charts';
	import ChartPane from './chart-pane.svelte';
	import PaneHeader from './pane-header.svelte';
	import SubPaneChart from './sub-pane-chart.svelte';
	import SubPaneHeader from './sub-pane-header.svelte';
	import { splitPaneLayers, type DrawingRuntime, type LayerRuntime } from '$lib/charts/pane-runtime';
	import { linkTimeScales } from '$lib/charts/axis-sync';
	import type { ChartSettings } from '$lib/mock/chart-settings';
	import type { ToolKey } from './chart-config';

	let {
		instrument,
		spec,
		layers,
		drawings,
		settings,
		reloadToken,
		resetToken,
		onContextMenu,
		selectedDrawingId,
		showFocusRing,
		tool,
		replaying,
		replayCut,
		replayPct,
		clearToken,
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
		instrument: string;
		spec: string;
		layers: LayerRuntime[];
		drawings: DrawingRuntime[];
		/** This pane's chart settings, applied by the chart itself. */
		settings: ChartSettings;
		reloadToken: number;
		resetToken: number;
		onContextMenu?: (event: { x: number; y: number; region: 'chart' | 'price-scale' | 'time-scale' }) => void;
		selectedDrawingId: string | null;
		showFocusRing: boolean;
		tool: ToolKey;
		replaying: boolean;
		replayCut: number | null;
		replayPct: string;
		clearToken: number;
		onFocus: () => void;
		onToggleLayer: (layerId: string) => void;
		onOpenMainSettings: () => void;
		onOpenLayerSettings: (layerId: string) => void;
		onRemoveLayer: (layerId: string) => void;
		onLastClose?: (price: number) => void;
		onCreateDrawing: (drawing: Omit<DrawingRuntime, 'id' | 'position'>) => void;
		onSelectDrawing: (id: string | null) => void;
		onToolConsumed: () => void;
	} = $props();

	/** `null` while the pane's venue streams; otherwise why it does not. */
	let liveNotice = $state<string | null>(null);
	/** Where the pointer is, and which pane it is in. Mirrored to the others
	 * so a sub-pane under the price draws the same vertical line the main
	 * pane does — the pane the pointer is actually in is excluded, since
	 * lightweight-charts already draws its own crosshair there. */
	let crosshair = $state<{ source: string; time: UTCTimestamp | null }>({
		source: '',
		time: null
	});

	function crosshairFor(paneId: string): UTCTimestamp | null {
		return crosshair.source === paneId ? null : crosshair.time;
	}

	let hoverText = $state('');
	let narrow = $state(false);
	let priceScale = $state(0);
	let livePrice = $state<number | null>(null);

	const split = $derived(splitPaneLayers(layers));

	// Keeps the main chart's x-axis and every sub-pane indicator's own x-axis
	// moving together (the acceptance point: pan or zoom one, all
	// follow). Plain instance state, not `$state` — these are chart-library
	// handles wired up for their side effects, the same way `chart`/`series`
	// are plain fields in chart-pane.svelte itself, not reactive state to
	// render from.
	let mainChartApi: IChartApi | undefined;
	const subChartApis = new Map<string, IChartApi>();
	const subUnlinks = new Map<string, () => void>();

	function relinkSub(layerId: string) {
		subUnlinks.get(layerId)?.();
		subUnlinks.delete(layerId);
		const sub = subChartApis.get(layerId);
		if (mainChartApi && sub) {
			subUnlinks.set(layerId, linkTimeScales(mainChartApi, sub));
		}
	}

	// Adding or removing the *first*/*last* sub-pane layer destroys and
	// recreates the main chart too (pane-cell's own `{#if split.sub.length
	// === 0}` branch switch, see `chart-pane.svelte`'s `destroyed` doc), so
	// every existing sub-pane must be relinked against the fresh main chart
	// instance, not only the one that just (re)mounted.
	function handleMainChartApi(chart: IChartApi | undefined) {
		mainChartApi = chart;
		for (const layerId of subChartApis.keys()) relinkSub(layerId);
	}

	function handleSubChartApi(layerId: string, chart: IChartApi | undefined) {
		if (chart) subChartApis.set(layerId, chart);
		else subChartApis.delete(layerId);
		relinkSub(layerId);
	}
</script>

{#snippet mainChart()}
	<div class="relative h-full min-h-0" style="box-shadow: {showFocusRing ? 'inset 0 0 0 1px rgba(var(--ink),0.28)' : 'none'};">
		<ChartPane
			{instrument}
			{spec}
			{tool}
			replayIdx={replayCut}
			{clearToken}
			overlayLayers={split.main}
			{drawings}
			{settings}
			{reloadToken}
			{resetToken}
			{onContextMenu}
			{selectedDrawingId}
			onCrosshair={(t) => (hoverText = t)}
			onNarrow={(n) => (narrow = n)}
			onLastClose={(price) => onLastClose?.(price)}
			onLiveNotice={(notice) => (liveNotice = notice)}
			onPriceScale={(scale) => (priceScale = scale)}
			onLivePrice={(price) => (livePrice = price)}
			crosshairTime={crosshairFor('main')}
			onCrosshairTime={(time) => (crosshair = { source: 'main', time })}
			onChartApi={handleMainChartApi}
			{onCreateDrawing}
			{onSelectDrawing}
			{onToolConsumed}
		/>
		<PaneHeader
			{liveNotice}
			{instrument}
			{spec}
			layers={split.main}
			{hoverText}
			{narrow}
			{livePrice}
			{onToggleLayer}
			{onOpenMainSettings}
			{onOpenLayerSettings}
			{onRemoveLayer}
		/>
		{#if replaying}
			<div class="absolute bottom-2.5 left-2.5 z-[8] flex items-center gap-2 border border-ink/14 bg-bg2 px-2.5 py-1">
				<div class="size-[5px] rounded-full bg-gain" style="animation: pane-cell-pulse 1.2s infinite;"></div>
				<span class="font-mono text-[9px] tracking-[0.2em] text-secondary-foreground">REPLAY {replayPct}</span>
			</div>
		{/if}
	</div>
{/snippet}

<div
	role="button"
	tabindex="0"
	class="relative flex h-full min-h-0 w-full min-w-0 flex-col bg-bg2"
	onclick={onFocus}
	onkeydown={(e) => {
		if (e.key === 'Enter' || e.key === ' ') onFocus();
	}}
>
	{#if split.sub.length === 0}
		<div class="min-h-0 flex-1">
			{@render mainChart()}
		</div>
	{:else}
		<Resizable.PaneGroup direction="vertical" class="min-h-0 flex-1">
			<Resizable.Pane defaultSize={split.sub.length > 1 ? 55 : 65} minSize={25}>
				{@render mainChart()}
			</Resizable.Pane>
			{#each split.sub as sl (sl.id)}
				<Resizable.Handle />
				<Resizable.Pane defaultSize={45 / split.sub.length} minSize={12}>
					<div class="relative h-full min-h-0 border-t border-border">
						<SubPaneChart
							{instrument}
							{spec}
							layer={sl}
							{priceScale}
							replayIdx={replayCut}
							crosshairTime={crosshairFor(sl.id)}
							onCrosshairTime={(time) => (crosshair = { source: sl.id, time })}
							onChartApi={(chart) => handleSubChartApi(sl.id, chart)}
						/>
						<SubPaneHeader layer={sl} {onToggleLayer} onOpenSettings={(id) => onOpenLayerSettings(id)} {onRemoveLayer} />
					</div>
				</Resizable.Pane>
			{/each}
		</Resizable.PaneGroup>
	{/if}
</div>

<style>
	@keyframes pane-cell-pulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.35;
		}
	}
</style>
