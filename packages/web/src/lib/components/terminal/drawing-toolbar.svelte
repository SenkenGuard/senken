<script lang="ts">
	// The small floating bar TradingView shows beside a selected drawing
	// object instead of a properties dialog — colour, line style, line width,
	// delete. Reuses `drawing-dialog.svelte`'s controls (the same
	// `ColorPicker`, the same `LINE_STYLES`/width-button row) since the
	// underlying edit is identical; only the shell changes from a modal to a
	// bar the caller positions next to the object itself.
	//
	// Props in, events out: this component holds no state of its own and
	// never touches `workspace-store.svelte.ts` — the caller supplies the
	// drawing's current style and where to place the bar, and decides what
	// happens with a style change or a delete.
	import { ColorPicker } from '$lib/components/ui/color-picker/index.js';
	import { cn } from '$lib/utils.js';
	import { LINE_STYLES, type LineStyle } from '$lib/mock/chart-settings';
	import type { DrawingRuntime } from '$lib/charts/pane-runtime';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';

	const WIDTHS = [1, 2, 3, 4];

	let {
		x,
		y,
		color,
		width,
		lineStyle,
		onChangeStyle,
		onDelete
	}: {
		/** Where to anchor the bar, in the same pixel space as the chart
		 * pane's own container — typically just above the selected drawing's
		 * body or first handle. The bar centers itself horizontally on this
		 * point and sits just above it. */
		x: number;
		y: number;
		color: string;
		width: number;
		lineStyle: LineStyle;
		onChangeStyle: (patch: Partial<Pick<DrawingRuntime, 'color' | 'width' | 'lineStyle'>>) => void;
		onDelete: () => void;
	} = $props();
</script>

<div
	class="absolute z-20 flex -translate-x-1/2 -translate-y-full items-center gap-2 border border-ink/18 bg-card2 px-2 py-[7px] shadow-[0_20px_48px_rgba(0,0,0,0.7)]"
	style={`left:${x}px; top:${y - 10}px;`}
>
	<ColorPicker value={color} fallback="#f2f2ef" onChange={(hex) => onChangeStyle({ color: hex })} />

	<div class="flex gap-[3px]">
		{#each LINE_STYLES as s (s)}
			<button
				type="button"
				class={cn('cursor-pointer border px-[7px] py-[3px]', lineStyle === s ? 'border-foreground bg-foreground' : 'border-ink/14')}
				onclick={() => onChangeStyle({ lineStyle: s })}
			>
				<span class={cn('font-mono text-[8px] tracking-[0.12em]', lineStyle === s ? 'text-inv' : 'text-dim2')}>{s}</span>
			</button>
		{/each}
	</div>

	<div class="flex gap-[3px]">
		{#each WIDTHS as w (w)}
			<button
				type="button"
				class={cn('w-[20px] cursor-pointer border py-[3px] text-center', width === w ? 'border-foreground bg-foreground' : 'border-ink/14')}
				onclick={() => onChangeStyle({ width: w })}
			>
				<span class={cn('font-mono text-[8px]', width === w ? 'text-inv' : 'text-dim2')}>{w}</span>
			</button>
		{/each}
	</div>

	<button
		type="button"
		class="flex cursor-pointer items-center border border-ink/14 px-[7px] py-[4px] text-destructive"
		onclick={onDelete}
		aria-label="Delete drawing"
	>
		<Trash2Icon class="size-3" />
	</button>
</div>
