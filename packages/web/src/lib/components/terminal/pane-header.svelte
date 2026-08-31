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
		hoverText,
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
		/** `null` while this pane's venue streams; otherwise why it does not. */
		liveNotice?: string | null;
		layers: LayerRuntime[];
		/** Layers whose series is being recomputed. The plot already drawn
		 * stays put; this only says the layer is working. */
		loadingLayerIds?: string[];
		hoverText: string;
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

	function layerLabel(layer: LayerRuntime): string {
		return layer.kind === 'overlay_instrument' ? parseInstrumentId(layer.instrument ?? '').ticker : (layer.indicatorName ?? '');
	}
	function layerParams(layer: LayerRuntime): string {
		if (layer.kind === 'overlay_instrument') return 'overlay';
		const entries = Object.entries(layer.params);
		return entries.length === 0 ? '' : entries.map(([, v]) => v).join(' · ');
	}
</script>

<div class="pointer-events-none absolute top-[7px] left-2 z-[6] flex flex-col items-start gap-px overflow-hidden" style="right: 68px;">
	<!-- The pane's own main instrument — always present, never removable. -->
	<div class="group/chip pointer-events-auto flex w-fit max-w-full min-w-0 items-center gap-1.5 py-0.5 pr-1.5 pl-0.5 hover:bg-ink/7">
		{#if liveNotice}
			<div class="size-[5px] flex-none rounded-full bg-dim2" title={liveNotice}></div>
		{:else}
			<div class="size-[5px] flex-none rounded-full bg-gain" style="animation: senken-pulse 2.4s infinite;"></div>
		{/if}
		<span class="min-w-0 truncate font-mono text-[11px] font-medium tracking-[0.04em] whitespace-nowrap">
			{#if !narrow}<span class="text-dim2">{main.venue}:</span>{/if}<span class="text-foreground">{main.ticker}</span>
		</span>
		<span class="min-w-0 truncate font-mono text-[9px] whitespace-nowrap text-dim2 uppercase">{spec}</span>
		<span class="flex-none border border-ink/16 px-[3px] font-mono text-[7.5px] tracking-[0.14em] text-dim2">MAIN</span>
		{#if liveNotice}
			<span
				class="flex-none border border-ink/16 px-[3px] font-mono text-[7.5px] tracking-[0.14em] text-dim2"
				title={liveNotice}>NO FEED</span
			>
		{/if}
		{#if hoverText}
			<span class="min-w-0 truncate font-mono text-[9.5px] whitespace-nowrap text-secondary-foreground">{hoverText}</span>
		{:else if livePrice != null}
			<span class="min-w-0 truncate font-mono text-[9.5px] whitespace-nowrap text-secondary-foreground">{livePrice.toFixed(4)}</span>
		{/if}
		<button
			type="button"
			class="hidden flex-none cursor-pointer text-secondary-foreground group-hover/chip:block"
			onclick={onOpenMainSettings}
			aria-label={`${main.ticker} settings`}
		>
			<SettingsIcon class="size-3" />
		</button>
	</div>

	{#each layers as l (l.id)}
		{@const Icon = LAYER_KIND_ICON[l.kind]}
		{@const on = l.visible}
		<div class="group/chip pointer-events-auto flex w-fit max-w-full min-w-0 items-center gap-1.5 py-0.5 pr-1.5 pl-0.5 hover:bg-ink/7">
			{#if loadingLayerIds.includes(l.id)}
				<LoaderIcon class="size-3 flex-none animate-spin text-dim2" />
			{:else}
				<Icon class={cn('size-3 flex-none', on ? 'text-secondary-foreground' : 'text-dim')} />
			{/if}
			<span class="min-w-0 truncate font-mono text-[11px] font-medium tracking-[0.04em] whitespace-nowrap">
				<span class={on ? 'text-secondary-foreground' : 'text-dim'}>{layerLabel(l)}</span>
			</span>
			<span class="min-w-0 truncate font-mono text-[9px] whitespace-nowrap text-dim2">{layerParams(l)}</span>
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
