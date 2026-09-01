<script lang="ts">
	// Per-pane layer-chip overlay: the
	// main instrument chip (venue:ticker, timeframe, MAIN badge, live dot,
	// crosshair status line) stacked with any overlay instrument/indicator
	// layers. Sits absolutely positioned over chart-pane's canvas, exactly as
	// in the reference — this and chart-pane share one relative parent
	// (pane-grid.svelte).
	//
	// the main instrument is the pane's own `instrument`/`spec`
	// fields, not a layer — `senken_workspace::LayerKind` never models "the
	// pane's own instrument" (see `$lib/charts/pane-runtime.ts`'s header
	// comment) — so this component takes them as separate props instead of
	// finding a `primary: true` layer in the list the way the former mock
	// model did. The main chip's settings icon opens chart settings (there
	// is no "layer" to open a layer dialog for); every other chip here is a
	// real layer (`overlay_instrument`/`indicator_overlay`) and can be
	// toggled/edited/removed like any other, per its toggle/settings/remove
	// set.
	import { cn } from '$lib/utils.js';
	import { parseInstrumentId, LAYER_KIND_ICON } from './chart-config';
	import type { LayerRuntime } from '$lib/charts/pane-runtime';
	import { statusLineSegments, type StatusBar } from '$lib/charts/status-line';
	import type { ChartSettings } from '$lib/mock/chart-settings';
	import EyeIcon from '@lucide/svelte/icons/eye';
	import LoaderIcon from '@lucide/svelte/icons/loader-circle';
	import EyeOffIcon from '@lucide/svelte/icons/eye-off';
	import SettingsIcon from '@lucide/svelte/icons/settings-2';
	import XIcon from '@lucide/svelte/icons/x';

	let {
		instrument,
		spec,
		layers,
		loadingLayerIds = [],
		barsLoading = false,
		statusBar,
		settings,
		narrow,
		livePrice,
		liveNotice,
		onToggleLayer,
		onOpenMainSettings,
		onOpenLayerSettings,
		onRemoveLayer
	}: {
		instrument: string;
		spec: string;
		/** `null` while this pane's venue streams; otherwise the chip's short
		 * label and the sentence behind it — "MARKET CLOSED" and "NO LIVE
		 * FEED" are different facts and this shows whichever one is true. */
		liveNotice?: { label: string; message: string } | null;
		layers: LayerRuntime[];
		/** Layers whose series is being recomputed. The plot already drawn
		 * stays put; this only says the layer is working. */
		loadingLayerIds?: string[];
		/** Whether the pane's own bars are being fetched. Shown on the instrument
		 * row for the same reason each layer shows its own: a chart that is
		 * working looks identical to one that has stalled. */
		barsLoading?: boolean;
		/** The bar the status line describes, or `null` while the pane has no
		 * bars. */
		statusBar: StatusBar | null;
		/** This pane's stored settings — the status-line group decides which
		 * parts of this header are drawn at all. */
		settings: ChartSettings;
		narrow: boolean;
		/** The live last-traded price, shown in place of the
		 * crosshair text whenever the cursor isn't over the chart — `null`
		 * before the first tick arrives for a freshly (re)subscribed
		 * instrument. */
		livePrice: number | null;
		onToggleLayer: (id: string) => void;
		onOpenMainSettings: () => void;
		onOpenLayerSettings: (layerId: string) => void;
		onRemoveLayer: (layerId: string) => void;
	} = $props();

	const main = $derived(parseInstrumentId(instrument));

	/** The header's own width, measured rather than passed down: it is the
	 * only thing that decides whether four price cells fit, and it is right
	 * here. `narrow` (the venue prefix) stays a prop because the chart
	 * reports it against its own container, not this overlay.
	 *
	 * Measured on the strip the chips actually get — the pane minus the
	 * price axis — so it drops to the close a little earlier than the old
	 * whole-pane check did. Deliberately: the line can now carry a change
	 * and a volume cell as well, which the old check knew nothing about. */
	let headerWidth = $state(0);

	/** How many decimal places a price gets. The pane's own setting when it
	 * names one; otherwise four, which is what this header has always
	 * shown. There is no `price_scale` in this component, and guessing one
	 * would be worse than a fixed fallback. */
	const precision = $derived(settings.precision === 'DEFAULT' ? 4 : Number(settings.precision));

	const segments = $derived(
		statusBar
			? statusLineSegments(statusBar, settings, { precision, compact: headerWidth < 420 })
			: []
	);

	const toneClass = { up: 'text-gain', down: 'text-loss', neutral: 'text-secondary-foreground' } as const;

	function layerLabel(layer: LayerRuntime): string {
		return layer.kind === 'overlay_instrument' ? parseInstrumentId(layer.instrument ?? '').ticker : (layer.indicatorName ?? '');
	}
	function layerParams(layer: LayerRuntime): string {
		if (layer.kind === 'overlay_instrument') return 'overlay';
		const entries = Object.entries(layer.params);
		return entries.length === 0 ? '' : entries.map(([, v]) => v).join(' · ');
	}
</script>

<div
	bind:clientWidth={headerWidth}
	class="pointer-events-none absolute top-[7px] left-2 z-[6] flex flex-col items-start gap-px overflow-hidden"
	style="right: 68px;"
>
	<!-- The pane's own main instrument — always present, never removable. -->
	<div class="group/chip pointer-events-auto flex w-fit max-w-full min-w-0 items-center gap-1.5 py-0.5 pr-1.5 pl-0.5 hover:bg-ink/7">
		{#if liveNotice}
			<div class="size-[5px] flex-none rounded-full bg-dim2" title={liveNotice.message}></div>
		{:else}
			<div class="size-[5px] flex-none rounded-full bg-gain" style="animation: senken-pulse 2.4s infinite;"></div>
		{/if}
		<!-- The instrument's own name and timeframe, hidden when the reader
		     turns the status line's title off. The chip itself stays: it
		     carries this pane's settings button and its live-state dot. -->
		{#if settings.statusSymbol}
			<span class="min-w-0 truncate font-mono text-[11px] font-medium tracking-[0.04em] whitespace-nowrap">
				{#if !narrow}<span class="text-dim2">{main.venue}:</span>{/if}<span class="text-foreground">{main.ticker}</span>
			</span>
			<span class="min-w-0 truncate font-mono text-[9px] whitespace-nowrap text-dim2 uppercase">{spec}</span>
			<span class="flex-none border border-ink/16 px-[3px] font-mono text-[7.5px] tracking-[0.14em] text-dim2">MAIN</span>
		{/if}
		<!-- Whichever fact is actually true — a venue with no stream and a
		     market that is simply closed are different things, and this used
		     to print the first for both. -->
		{#if liveNotice}
			<span
				class="flex-none border border-ink/16 px-[3px] font-mono text-[7.5px] tracking-[0.14em] whitespace-nowrap text-dim2"
				title={liveNotice.message}>{liveNotice.label}</span
			>
		{/if}
		{#each segments as segment, i (segment.label + i)}
			<span class="flex min-w-0 flex-none items-baseline gap-1 font-mono text-[9.5px] whitespace-nowrap">
				{#if segment.label}<span class="text-dim">{segment.label}</span>{/if}
				<span class={toneClass[segment.tone]}>{segment.value}</span>
			</span>
		{/each}
		{#if segments.length === 0 && livePrice != null}
			<span class="min-w-0 truncate font-mono text-[9.5px] whitespace-nowrap text-secondary-foreground">{livePrice.toFixed(precision)}</span>
		{/if}
		<!-- While it is working, the spinner takes the action's place rather than
		     sitting beside it: two controls where there was one shifts the row
		     under the cursor, and an action pressed mid-fetch acts on a state
		     that is about to be replaced. -->
		{#if barsLoading}
			<LoaderIcon class="size-3 flex-none animate-spin text-dim2" data-loading="bars" />
		{:else}
			<button
				type="button"
				class="hidden flex-none cursor-pointer text-secondary-foreground group-hover/chip:block"
				onclick={onOpenMainSettings}
				aria-label={`${main.ticker} settings`}
			>
				<SettingsIcon class="size-3" />
			</button>
		{/if}
	</div>

	<!-- Indicator/overlay chips, hidden when the reader turns indicator
	     titles off. They are the only place a layer can be toggled, opened
	     or removed *on the chart*, so the object-tree panel keeps every one
	     of those actions reachable either way. -->
	{#each settings.statusValues ? layers : [] as l (l.id)}
		{@const Icon = LAYER_KIND_ICON[l.kind]}
		{@const on = l.visible}
		<div class="group/chip pointer-events-auto flex w-fit max-w-full min-w-0 items-center gap-1.5 py-0.5 pr-1.5 pl-0.5 hover:bg-ink/7">
			<Icon class={cn('size-3 flex-none', on ? 'text-secondary-foreground' : 'text-dim')} />
			<span class="min-w-0 truncate font-mono text-[11px] font-medium tracking-[0.04em] whitespace-nowrap">
				<span class={on ? 'text-secondary-foreground' : 'text-dim'}>{layerLabel(l)}</span>
			</span>
			{#if settings.statusInputs}
				<span class="min-w-0 truncate font-mono text-[9px] whitespace-nowrap text-dim2">{layerParams(l)}</span>
			{/if}
			<!-- The layer is working: its actions step aside for the spinner, so the
			     row keeps its width and nothing can be pressed against a series that
			     is about to be replaced. -->
			{#if loadingLayerIds.includes(l.id)}
				<LoaderIcon class="size-3 flex-none animate-spin text-dim2" data-loading={l.id} />
			{:else}
				<button
					type="button"
					class="hidden flex-none cursor-pointer text-secondary-foreground group-hover/chip:block"
					onclick={() => onToggleLayer(l.id)}
					aria-label={on ? `Hide ${layerLabel(l)}` : `Show ${layerLabel(l)}`}
				>
					{#if on}
						<EyeIcon class="size-3" />
					{:else}
						<EyeOffIcon class="size-3" />
					{/if}
				</button>
				<button
					type="button"
					class="hidden flex-none cursor-pointer text-secondary-foreground group-hover/chip:block"
					onclick={() => onOpenLayerSettings(l.id)}
					aria-label={`${layerLabel(l)} settings`}
				>
					<SettingsIcon class="size-3" />
				</button>
				<button
					type="button"
					class="hidden flex-none cursor-pointer text-secondary-foreground group-hover/chip:block"
					onclick={() => onRemoveLayer(l.id)}
					aria-label={`Remove ${layerLabel(l)}`}
				>
					<XIcon class="size-3" />
				</button>
			{/if}
		</div>
	{/each}
</div>

<style>
	@keyframes senken-pulse {
		0%,
		100% {
			opacity: 1;
		}
		50% {
			opacity: 0.35;
		}
	}
</style>
