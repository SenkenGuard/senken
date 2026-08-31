<script lang="ts">
	// Layer dialog.
	// Domain-specific (edits one chart pane's layer), so it lives in
	// `terminal/` per Part C. Reachable only from the charts route —
	// `routes/charts/+page.svelte` owns the open/closed `layerDlg` state
	// locally, same as before.
	//
	// `senken_workspace::LayerKind` only ever models an
	// overlaid instrument or an indicator (name + JSON params) — there is no
	// "strategy" layer kind, and no per-timeframe visibility field (the
	// former mock's VISIBILITY tab has nowhere to persist to, so it is
	// dropped here rather than kept as UI that edits nothing — see this
	// milestone's report). Two tabs remain:
	// - INPUTS: one stepper row per key in the layer's `params` (its real,
	//   persisted construction parameters — the "input settings").
	// - STYLE: color/width/line-style per plot the indicator reports — kept
	//   local-only (`$lib/charts/layer-style.ts`), since no server field
	//   carries it yet (the "say which").
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import { ColorPicker } from '$lib/components/ui/color-picker/index.js';
	import { cn } from '$lib/utils.js';
	import { LAYER_KIND_ICON, parseInstrumentId } from './chart-config';
	import { plotsForLayer, setPlotStyle, type PlotStyle } from '$lib/charts/layer-style';
	import { LINE_STYLES } from '$lib/mock/chart-settings';
	import type { LayerRuntime } from '$lib/charts/pane-runtime';
	import CheckIcon from '@lucide/svelte/icons/check';
	import MinusIcon from '@lucide/svelte/icons/minus';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import XIcon from '@lucide/svelte/icons/x';

	let {
		layer,
		onClose,
		onEditParams,
		onStyleChanged
	}: {
		layer: LayerRuntime | null;
		onClose: () => void;
		onEditParams: (patch: Record<string, number>) => void;
		/** Fired after a plot's styling changed, so the caller can persist
		 * the layout the styling now rides on. */
		onStyleChanged?: () => void;
	} = $props();

	let tab = $state<'INPUTS' | 'STYLE'>('INPUTS');

	let lastLayerId = $state<string | null>(null);
	$effect(() => {
		if (layer && layer.id !== lastLayerId) {
			lastLayerId = layer.id;
			tab = layer.kind === 'overlay_instrument' ? 'INPUTS' : 'STYLE';
		}
	});

	const Icon = $derived(layer ? LAYER_KIND_ICON[layer.kind] : null);
	const title = $derived(layer ? (layer.kind === 'overlay_instrument' ? parseInstrumentId(layer.instrument ?? '').ticker : (layer.indicatorName ?? '')) : '');

	/** Step size per parameter key — `k` (Bollinger's std-dev width) is the
	 * one fractional parameter the ten built-ins take; everything else is a
	 * bar-count period. */
	function stepFor(key: string): number {
		return key === 'k' ? 0.5 : 1;
	}

	/** The inputs as the user is editing them, which is not the same thing as
	 * the inputs the chart is currently drawn with: a period is typed one
	 * digit at a time, and recomputing on every keystroke would redraw the
	 * series for values like `1` and `14` on the way to `140`. Committed on a
	 * debounce instead, so several inputs can be changed in one go. */
	let draft = $state<Record<string, number>>({});
	let editing = $state<Record<string, string>>({});
	let commitTimer: ReturnType<typeof setTimeout> | undefined;

	/** Reseeded when a *different* layer is opened, not whenever the layer
	 * object changes: saving re-reads the layout, and re-seeding on that
	 * would overwrite what the user is in the middle of typing. */
	let seededFor = $state<string | null>(null);
	$effect(() => {
		const key = layer ? `${layer.position}:${layer.indicatorName ?? layer.kind}` : null;
		if (key === seededFor) return;
		seededFor = key;
		draft = layer ? { ...layer.params } : {};
		editing = Object.fromEntries(Object.entries(draft).map(([k, v]) => [k, String(v)]));
		// A dialog reopened after a visit to STYLE should still land on the
		// inputs: that is what someone opening an indicator's settings is
		// nearly always after.
		if (layer) tab = 'INPUTS';
	});

	function scheduleCommit() {
		if (commitTimer !== undefined) clearTimeout(commitTimer);
		commitTimer = setTimeout(() => {
			commitTimer = undefined;
			const changed = Object.fromEntries(
				Object.entries(draft).filter(([k, v]) => layer && layer.params[k] !== v)
			);
			if (Object.keys(changed).length > 0) onEditParams(changed);
		}, 450);
	}

	function setParam(key: string, value: number) {
		draft = { ...draft, [key]: value };
		editing = { ...editing, [key]: String(value) };
		scheduleCommit();
	}

	function typeParam(key: string, text: string) {
		editing = { ...editing, [key]: text };
		const parsed = Number(text);
		if (text.trim() === '' || Number.isNaN(parsed) || parsed <= 0) return;
		draft = { ...draft, [key]: parsed };
		scheduleCommit();
	}

	function inputRows(): {
		key: string;
		label: string;
		text: string;
		onInc: () => void;
		onDec: () => void;
	}[] {
		return Object.entries(draft).map(([key, value]) => ({
			key,
			label: key.toUpperCase().replaceAll('_', ' '),
			text: editing[key] ?? String(value),
			onInc: () => setParam(key, value + stepFor(key)),
			onDec: () => setParam(key, Math.max(stepFor(key), value - stepFor(key)))
		}));
	}

	let plots = $state<PlotStyle[]>([]);
	$effect(() => {
		plots = layer && layer.indicatorName ? plotsForLayer(layer.id, layer.indicatorName) : [];
	});

	function patchPlot(field: string, patch: Partial<PlotStyle>) {
		if (!layer || !layer.indicatorName) return;
		setPlotStyle(layer.id, layer.indicatorName, field, patch);
		plots = plotsForLayer(layer.id, layer.indicatorName);
		// Styling rides with the layout, so a colour or line width outlives
		// the session the same way a period does.
		onStyleChanged?.();
	}
</script>

<Dialog.Root
	open={!!layer}
	onOpenChange={(v) => {
		if (!v) onClose();
	}}
>
	<Dialog.Content
		showCloseButton={false}
		class="top-[74px] flex max-h-[74%] w-[452px] max-w-[92%] translate-y-0 flex-col gap-0 overflow-hidden border-ink/18 bg-card2 p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		{#if layer}
			<Dialog.Header class="flex-row items-center justify-between gap-3 space-y-0 px-4 py-[13px]">
				<div class="flex min-w-0 items-center gap-[9px]">
					{#if Icon}<Icon class="size-[15px] flex-none text-dim2" />{/if}
					<Dialog.Title class="truncate text-[13px] font-medium tracking-[0.03em] text-foreground">{title}</Dialog.Title>
				</div>
				<Dialog.Close class="flex-none cursor-pointer text-dim2" aria-label="Close">
					<XIcon class="size-[14px]" />
				</Dialog.Close>
			</Dialog.Header>
			<Dialog.Description class="sr-only">Edit {title} settings</Dialog.Description>

			<Tabs.Root bind:value={tab} class="flex min-h-0 flex-1 flex-col">
				<Tabs.List variant="line" class="h-auto flex-none justify-start gap-[18px] rounded-none border-b border-ink/10 bg-transparent px-4 py-0">
					{#each ['INPUTS', 'STYLE'] as const as t (t)}
						<Tabs.Trigger
							value={t}
							class="h-auto flex-none rounded-none border-0 bg-transparent p-0 pb-[9px] font-mono text-[9.5px] tracking-[0.18em] text-dim data-[state=active]:bg-transparent data-[state=active]:text-foreground data-[state=active]:shadow-none"
						>
							{t}
						</Tabs.Trigger>
					{/each}
				</Tabs.List>

				<div class="min-h-0 flex-1 overflow-auto px-4 pt-[14px] pb-4">
					<Tabs.Content value="INPUTS" class="mt-0 flex flex-col gap-[11px]">
						{#if layer.kind === 'overlay_instrument'}
							<div class="flex items-center justify-between gap-3.5">
								<span class="font-mono text-[9.5px] tracking-[0.16em] text-dim2">INSTRUMENT</span>
								<span class="font-mono text-[11px] text-foreground">{parseInstrumentId(layer.instrument ?? '').venue}:{parseInstrumentId(layer.instrument ?? '').ticker}</span>
							</div>
						{:else}
							{#each inputRows() as f (f.key)}
								<div class="flex items-center justify-between gap-3.5">
									<span class="font-mono text-[9.5px] tracking-[0.16em] text-dim2">{f.label}</span>
									<div class="flex items-center gap-1.5">
										<button type="button" class="flex h-[26px] w-6 cursor-pointer items-center justify-center border border-ink/14 text-dim2" onclick={f.onDec} aria-label="Decrease {f.label}">
											<MinusIcon class="size-[11px]" />
										</button>
										<input
											type="text"
											inputmode="decimal"
											class="h-[26px] min-w-[92px] border border-ink/14 bg-transparent px-2.5 text-center font-mono text-[11px] text-foreground outline-none focus:border-foreground"
											aria-label={f.label}
											value={f.text}
											oninput={(e) => typeParam(f.key, e.currentTarget.value)}
										/>
										<button type="button" class="flex h-[26px] w-6 cursor-pointer items-center justify-center border border-ink/14 text-dim2" onclick={f.onInc} aria-label="Increase {f.label}">
											<PlusIcon class="size-[11px]" />
										</button>
									</div>
								</div>
							{/each}
							{#if inputRows().length === 0}
								<p class="font-mono text-[9.5px] text-dim">This indicator takes no parameters.</p>
							{/if}
						{/if}
					</Tabs.Content>

					<Tabs.Content value="STYLE" class="mt-0 flex flex-col gap-4">
						{#each plots as p (p.field)}
							<div class="flex flex-col gap-2.5">
								<div class="flex items-center gap-2.5">
									<button
										type="button"
										class={cn('flex size-[15px] cursor-pointer items-center justify-center border', p.visible ? 'border-foreground bg-foreground' : 'border-ink/24')}
										onclick={() => patchPlot(p.field, { visible: !p.visible })}
										aria-label={p.visible ? 'Hide plot' : 'Show plot'}
									>
										{#if p.visible}<CheckIcon class="size-2.5 text-inv" />{/if}
									</button>
									<span class="font-mono text-[10.5px] tracking-[0.14em] text-foreground">{p.label}</span>
									<span class="font-mono text-[8.5px] tracking-[0.16em] text-dim">{p.type.toUpperCase()}</span>
								</div>
								<div class="flex flex-col gap-[9px] pl-[25px]">
									<div class="flex items-center gap-2.5">
										<span class="flex-none basis-[74px] font-mono text-[8.5px] tracking-[0.16em] text-dim">COLOR</span>
										<ColorPicker value={p.color} fallback="#f2f2ef" onChange={(hex) => patchPlot(p.field, { color: hex })} />
									</div>
									{#if p.type === 'line'}
										<div class="flex items-center gap-2.5">
											<span class="flex-none basis-[74px] font-mono text-[8.5px] tracking-[0.16em] text-dim">LINE</span>
											<div class="flex gap-[5px]">
												{#each LINE_STYLES as s (s)}
													<button
														type="button"
														class={cn('cursor-pointer border px-[9px] py-1', p.style === s ? 'border-foreground bg-foreground' : 'border-ink/14')}
														onclick={() => patchPlot(p.field, { style: s })}
													>
														<span class={cn('font-mono text-[8.5px] tracking-[0.14em]', p.style === s ? 'text-inv' : 'text-dim2')}>{s}</span>
													</button>
												{/each}
											</div>
											<div class="ml-2 flex gap-[5px]">
												{#each [1, 2, 3] as w (w)}
													<button
														type="button"
														class={cn('w-[26px] cursor-pointer border py-1 text-center', Math.round(p.width) === w ? 'border-foreground bg-foreground' : 'border-ink/14')}
														onclick={() => patchPlot(p.field, { width: w })}
													>
														<span class={cn('font-mono text-[8.5px]', Math.round(p.width) === w ? 'text-inv' : 'text-dim2')}>{w}</span>
													</button>
												{/each}
											</div>
										</div>
									{/if}
								</div>
							</div>
						{/each}
						{#if plots.length === 0}
							<p class="font-mono text-[9.5px] text-dim">This layer has no plots to style.</p>
						{/if}
					</Tabs.Content>
				</div>
			</Tabs.Root>

			<div class="flex flex-none items-center justify-end gap-[7px] border-t border-ink/10 px-4 py-[11px]">
				<button type="button" class="cursor-pointer border border-ink/14 px-4 py-[7px]" onclick={onClose}>
					<span class="font-mono text-[9px] tracking-[0.18em] text-dim2">CLOSE</span>
				</button>
				<button type="button" class="cursor-pointer bg-foreground px-[18px] py-[7px]" onclick={onClose}>
					<span class="font-mono text-[9px] font-semibold tracking-[0.18em] text-inv">OK</span>
				</button>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
