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
	import { linkTimeScales, type LinkedTimeScales } from '$lib/charts/axis-sync';
	import type { ChartSettings } from '$lib/mock/chart-settings';
	import { cn } from '$lib/utils.js';
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
		onMoveDrawing,
		onPatchSettings,
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
		/** Patches this pane's chart settings — the price-scale shortcuts
		 * below write through it, the same path the settings dialog uses. */
		onPatchSettings?: (patch: Partial<ChartSettings>) => void;
		onMoveDrawing?: (
			id: string,
			patch: Partial<Pick<DrawingRuntime, 'price' | 'start' | 'end'>>
		) => void;
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
	/** Layers whose series is being recomputed, for the header's chips. */
	let loadingLayerIds = $state<string[]>([]);

	/** Remembers how the pane was split, debounced.
	 *
	 * The resizable group reports on every frame of a drag; writing each one
	 * would be a layout save per frame. Only the size the user settles on is
	 * worth keeping, and only if it actually differs from what is stored. */
	let splitTimer: ReturnType<typeof setTimeout> | undefined;
	function rememberSplit(sizes: number[]) {
		if (splitTimer !== undefined) clearTimeout(splitTimer);
		splitTimer = setTimeout(() => {
			splitTimer = undefined;
			const rounded = sizes.map((n) => Math.round(n * 10) / 10);
			const stored = settings.paneSplit ?? [];
			const same =
				stored.length === rounded.length && stored.every((n, i) => n === rounded[i]);
			if (!same) onPatchSettings?.({ paneSplit: rounded });
		}, 400);
	}
	let priceScale = $state(0);
	/** The main chart's own bar window, handed straight to every sub-pane so
	 * the strips below cannot start at a different bar than the candles. */
	let barTimes = $state<UTCTimestamp[]>([]);
	let livePrice = $state<number | null>(null);

	const split = $derived(splitPaneLayers(layers));

	// Keeps the main chart's x-axis and every sub-pane indicator's own x-axis
	// moving together (the acceptance point: pan or zoom one, all
	// follow). Plain instance state, not `$state` — these are chart-library
	// handles wired up for their side effects, the same way `chart`/`series`
	// are plain fields in chart-pane.svelte itself, not reactive state to
	// render from.
	let mainChartApi: IChartApi | undefined;
	// Keyed by the sub-pane's position, never by its layer id. Saving a
	// layout deletes and re-inserts every pane, layer and drawing, so all of
	// their ids change on any save — including one caused by drawing or
	// erasing a line, which has nothing to do with an indicator strip. Keyed
	// by id, that rebuilt every sub-pane chart on every drawing edit.
	const subChartApis = new Map<number, IChartApi>();
	const subLinks = new Map<number, LinkedTimeScales>();

	function relinkSub(subIndex: number) {
		subLinks.get(subIndex)?.dispose();
		subLinks.delete(subIndex);
		const sub = subChartApis.get(subIndex);
		if (mainChartApi && sub) {
			subLinks.set(subIndex, linkTimeScales(mainChartApi, sub));
		}
	}

	// Adding or removing the *first*/*last* sub-pane layer destroys and
	// recreates the main chart too (pane-cell's own `{#if split.sub.length
	// === 0}` branch switch, see `chart-pane.svelte`'s `destroyed` doc), so
	// every existing sub-pane must be relinked against the fresh main chart
	// instance, not only the one that just (re)mounted.
	function handleMainChartApi(chart: IChartApi | undefined) {
		mainChartApi = chart;
		for (const subIndex of subChartApis.keys()) relinkSub(subIndex);
		alignPriceScales();
	}

	function handleSubChartApi(subIndex: number, chart: IChartApi | undefined) {
		if (chart) subChartApis.set(subIndex, chart);
		else subChartApis.delete(subIndex);
		relinkSub(subIndex);
		alignPriceScales();
	}

	/** Makes every chart in this pane reserve the same width for its price
	 * scale.
	 *
	 * A sub-pane is the same bars as the main chart, drawn differently — so
	 * the same instant has to sit at the same x in both, or reading one
	 * against the other means nothing. They are separate chart instances
	 * sharing only a *logical* range, and a logical range maps to pixels
	 * through the plot width. An RSI axis labelled `90.00` is narrower than a
	 * price axis labelled `4444.71`, so the two plots end up different widths
	 * and the same bar lands in two places.
	 *
	 * The library's own note on `minimumWidth` names this case: a minimum
	 * width is how vertically stacked charts are given identical price
	 * scales. It is a floor, not a fixed size, so the widest scale is
	 * measured and every chart is given that as its minimum. */
	let alignTimer: ReturnType<typeof setTimeout> | undefined;
	function alignPriceScales() {
		if (alignTimer !== undefined) clearTimeout(alignTimer);
		// After the charts have laid out and measured their own labels.
		alignTimer = setTimeout(() => {
			alignTimer = undefined;
			const charts = [mainChartApi, ...subChartApis.values()].filter(
				(c): c is IChartApi => c !== undefined
			);
			if (charts.length < 2) return;
			const widest = Math.max(...charts.map((c) => c.priceScale('right').width()));
			for (const c of charts) {
				if (c.priceScale('right').options().minimumWidth !== widest) {
					c.priceScale('right').applyOptions({ minimumWidth: widest });
				}
			}
		}, 60);
	}
</script>

{#snippet mainChart()}
	<div class="relative h-full min-h-0" style="box-shadow: {showFocusRing ? 'inset 0 0 0 1px rgba(var(--ink),0.28)' : 'none'};">
		<!-- The price scale's own shortcuts: auto-fit and logarithmic, the two
		     a reader reaches for often enough that a right-click each time is
		     friction. Inside the axis gutter rather than over the plot, so
		     they cost no chart. Same state as the menu and the settings
		     dialog — all three write the pane's stored settings. -->
		<div class="pointer-events-auto absolute right-[9px] bottom-[28px] z-[7] flex gap-px">
			<button
				type="button"
				aria-label="Auto-fit price scale"
				title="AUTO FIT"
				class={cn(
					'flex h-[18px] w-[18px] cursor-pointer items-center justify-center border font-mono text-[9px]',
					settings.autoScale
						? 'border-foreground bg-foreground text-background'
						: 'border-ink/16 bg-popover text-dim2'
				)}
				onclick={() => onPatchSettings?.({ autoScale: !settings.autoScale })}
			>
				A
			</button>
			<button
				type="button"
				aria-label="Logarithmic price scale"
				title="LOGARITHMIC"
				class={cn(
					'flex h-[18px] w-[18px] cursor-pointer items-center justify-center border font-mono text-[9px]',
					settings.priceScaleMode === 'LOGARITHMIC'
						? 'border-foreground bg-foreground text-background'
						: 'border-ink/16 bg-popover text-dim2'
				)}
				onclick={() =>
					onPatchSettings?.({
						priceScaleMode: settings.priceScaleMode === 'LOGARITHMIC' ? 'REGULAR' : 'LOGARITHMIC'
					})}
			>
				L
			</button>
		</div>

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
			{onMoveDrawing}
			onLayerLoading={(ids) => (loadingLayerIds = ids)}
			onAutoScaleChanged={(autoScale) => onPatchSettings?.({ autoScale })}
			{selectedDrawingId}
			onCrosshair={(t) => (hoverText = t)}
			onNarrow={(n) => (narrow = n)}
			onLastClose={(price) => onLastClose?.(price)}
			onLiveNotice={(notice) => (liveNotice = notice)}
			onPriceScale={(scale) => (priceScale = scale)}
			onBarTimes={(times) => (barTimes = times)}
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
			{loadingLayerIds}
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
		<Resizable.PaneGroup
			direction="vertical"
			class="min-h-0 flex-1"
			onLayoutChange={(sizes) => rememberSplit(sizes)}
		>
			<Resizable.Pane
				defaultSize={settings.paneSplit?.[0] ?? (split.sub.length > 1 ? 55 : 65)}
				minSize={25}
			>
				{@render mainChart()}
			</Resizable.Pane>
			{#each split.sub as sl, subIndex (subIndex)}
				<Resizable.Handle />
				<Resizable.Pane
					defaultSize={settings.paneSplit?.[subIndex + 1] ?? 45 / split.sub.length}
					minSize={12}
				>
					<div class="relative h-full min-h-0 border-t border-border">
						<SubPaneChart
							{instrument}
							{spec}
							layer={sl}
							{settings}
							{priceScale}
							{barTimes}
							crosshairTime={crosshairFor(sl.id)}
							onCrosshairTime={(time) => (crosshair = { source: sl.id, time })}
							onChartApi={(chart) => handleSubChartApi(subIndex, chart)}
							onApplyPlots={(apply) => {
								const link = subLinks.get(subIndex);
								if (link) link.applyToFollower(apply);
								else apply();
							}}
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
