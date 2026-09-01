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
	import ChartPane from './chart-pane.svelte';
	import PaneHeader from './pane-header.svelte';
	import SubPaneHeader from './sub-pane-header.svelte';
	import { drawingsForInstrument, splitPaneLayers, type DrawingRuntime, type LayerRuntime } from '$lib/charts/pane-runtime';
	import type { ChartSettings } from '$lib/mock/chart-settings';
	import { cn } from '$lib/utils.js';
	import type { ToolKey } from './chart-config';
	import type { MarketStatus } from '$lib/charts/live-state';
	import type { StatusBar } from '$lib/charts/status-line';
	import type { IChartApi } from 'lightweight-charts';

	let {
		instrument,
		marketStatus,
		spec,
		layers,
		drawings,
		settings,
		reloadToken,
		resetToken,
		jumpTarget,
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
		onToolConsumed,
		onChartApi
	}: {
		instrument: string;
		/** The catalogued status for this instrument, when the page has one. */
		marketStatus?: MarketStatus;
		spec: string;
		layers: LayerRuntime[];
		drawings: DrawingRuntime[];
		/** This pane's chart settings, applied by the chart itself. */
		settings: ChartSettings;
		reloadToken: number;
		resetToken: number;
		/** A "go to date" request for this pane specifically, or `undefined`
		 * while none has been made. See `chart-pane.svelte`'s own doc. */
		jumpTarget?: { token: number; targetNanos: number } | null;
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
		/** Forwarded straight through from `chart-pane.svelte`'s own prop of
		 * the same name — this cell's `IChartApi`, once created, and
		 * `undefined` again on teardown. Lets the page hold one `IChartApi`
		 * per pane for the layout menu's TIME/CROSSHAIR sync toggles. */
		onChartApi?: (chart: IChartApi | undefined) => void;
	} = $props();

	/** `null` while the pane's venue streams; otherwise the header's chip
	 * label and the sentence behind it. */
	let liveNotice = $state<{ label: string; message: string } | null>(null);
	/** Where the pointer is, and which pane it is in. Mirrored to the others
	 * so a sub-pane under the price draws the same vertical line the main
	 * pane does — the pane the pointer is actually in is excluded, since
	 * lightweight-charts already draws its own crosshair there. */
	/** The bar the status line describes: under the crosshair while the
	 * pointer is on the plot, the newest loaded bar otherwise. */
	let statusBar = $state<StatusBar | null>(null);
	let narrow = $state(false);
	/** Layers whose series is being recomputed, for the header's chips. */
	let loadingLayerIds = $state<string[]>([]);
	/** Whether the pane's own bars are being fetched, for the instrument row. */
	let barsLoading = $state(false);

	let livePrice = $state<number | null>(null);

	const split = $derived(splitPaneLayers(layers));

	/** Only the drawings that belong to whatever this pane is showing right
	 * now. The pane keeps the rest — switching instrument and back brings
	 * them straight back — but a level drawn on one market must never be
	 * painted over another's candles. */
	const shownDrawings = $derived(drawingsForInstrument(drawings, instrument));

	/** Reported by the chart itself once its panes have been laid out. */
	let paneTops = $state<number[]>([]);

	function paneHeaderTop(subIndex: number): string {
		// Measured beats computed. The panes divide the chart area minus the
		// time axis and the separators between them, so no percentage of this
		// container lands on a pane's edge — which is why these headers sat
		// above the panes they label. The percentage below is only the first
		// frame, before the chart has reported anything.
		const measured = paneTops[subIndex];
		if (measured != null) return `${measured}px`;
		const stored = settings.paneSplit;
		if (stored?.length) return `${stored.slice(0, subIndex + 1).reduce((sum, size) => sum + size, 0)}%`;
		return `${65 + subIndex * (35 / split.sub.length)}%`;
	}

</script>

{#snippet mainChart()}
	<div class="relative h-full min-h-0" style="box-shadow: {showFocusRing ? 'inset 0 0 0 1px rgba(var(--ink),0.28)' : 'none'};">
		<!-- The price scale's own shortcuts: auto-fit and logarithmic, the two
		     a reader reaches for often enough that a right-click each time is
		     friction. Inside the axis gutter rather than over the plot, so
		     they cost no chart. Same state as the menu and the settings
		     dialog — all three write the pane's stored settings.
		     At the top of the gutter, not the bottom: the app's floating
		     assistant button is pinned to the window's bottom-right corner
		     and sits over that spot, which left these two unclickable in
		     every single-pane layout — the default one. The top of the
		     gutter is empty (the header chips stop well clear of it). -->
		<div class="pointer-events-auto absolute right-[9px] top-[7px] z-[7] flex gap-px">
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
			{marketStatus}
			{spec}
			{tool}
			replayIdx={replayCut}
			{clearToken}
			overlayLayers={split.main}
			drawings={shownDrawings}
			{settings}
			subPaneLayers={split.sub}
			{reloadToken}
			{resetToken}
			{jumpTarget}
			{onContextMenu}
			{onMoveDrawing}
			onLayerLoading={(ids) => (loadingLayerIds = ids)}
			onBarsLoading={(busy) => (barsLoading = busy)}
			onAutoScaleChanged={(autoScale) => onPatchSettings?.({ autoScale })}
			{selectedDrawingId}
			onStatusBar={(bar) => (statusBar = bar)}
			onNarrow={(n) => (narrow = n)}
			onLastClose={(price) => onLastClose?.(price)}
			onLiveNotice={(notice) => (liveNotice = notice)}
			onLivePrice={(price) => (livePrice = price)}
			onPaneTops={(tops) => (paneTops = tops)}
			{onCreateDrawing}
			{onSelectDrawing}
			{onToolConsumed}
			{onChartApi}
		/>
		<PaneHeader
			{liveNotice}
			{instrument}
			{spec}
			layers={split.main}
			{loadingLayerIds}
			{barsLoading}
			{statusBar}
			{settings}
			{narrow}
			{livePrice}
			{onToggleLayer}
			{onOpenMainSettings}
			{onOpenLayerSettings}
			{onRemoveLayer}
		/>
		{#if replaying}
			<div class="absolute right-2 bottom-7 z-[8] flex items-center gap-2 border border-ink/14 bg-bg2 px-2.5 py-1">
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
	<div class="relative min-h-0 flex-1">
		{@render mainChart()}
		{#each split.sub as sl, subIndex (sl.id)}
			<div
				class="pointer-events-none absolute right-16 left-0 z-[8]"
				style:top={paneHeaderTop(subIndex)}
			>
				<div class="pointer-events-auto">
					<SubPaneHeader layer={sl} loading={loadingLayerIds.includes(sl.id)} {onToggleLayer} onOpenSettings={(id) => onOpenLayerSettings(id)} {onRemoveLayer} />
				</div>
			</div>
		{/each}
	</div>
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
