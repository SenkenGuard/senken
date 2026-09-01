<script lang="ts">
	// The small floating bar shown beside a selected drawing object instead
	// of a properties modal — colour, line style, line width, show/hide,
	// delete, and (for a `text_note` only) its label. A modal would sit over
	// the very object being adjusted, which is why the bar exists at all.
	//
	// Props in, events out: this component holds no state of its own and
	// never touches `workspace-store.svelte.ts` — the caller supplies the
	// drawing's current style and where to place the bar, and decides what
	// happens with a style change or a delete.
	import { untrack } from 'svelte';
	import { ColorPicker } from '$lib/components/ui/color-picker/index.js';
	import { cn } from '$lib/utils.js';
	import { LINE_STYLES, type LineStyle } from '$lib/mock/chart-settings';
	import type { DrawingKindTag, DrawingRuntime } from '$lib/charts/pane-runtime';
	import { Input } from '$lib/components/ui/input/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import EyeIcon from '@lucide/svelte/icons/eye';
	import EyeOffIcon from '@lucide/svelte/icons/eye-off';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';

	const WIDTHS = [1, 2, 3, 4];

	let {
		x,
		y,
		kind,
		text,
		visible,
		color,
		width,
		lineStyle,
		onChangeStyle,
		onChangeText,
		onToggleVisible,
		onDelete
	}: {
		/** Where to anchor the bar, in the same pixel space as the chart
		 * pane's own container — typically just above the selected drawing's
		 * body or first handle. The bar centers itself horizontally on this
		 * point and sits just above it. */
		x: number;
		y: number;
		/** Which controls the bar shows besides style/visibility/delete — only
		 * `text_note` gets the text field, since it is the only kind with
		 * anything to type. */
		kind: DrawingKindTag;
		/** `text_note` only. */
		text?: string;
		visible: boolean;
		color: string;
		width: number;
		lineStyle: LineStyle;
		onChangeStyle: (patch: Partial<Pick<DrawingRuntime, 'color' | 'width' | 'lineStyle'>>) => void;
		/** Commits a `text_note`'s new label — called on blur and on Enter,
		 * never per keystroke, since each call is a server write
		 * (`updateDrawingText`, `workspace-store.svelte.ts`). */
		onChangeText: (text: string) => void;
		onToggleVisible: () => void;
		onDelete: () => void;
	} = $props();

	// Local draft so typing doesn't write on every keystroke — each commit is
	// a server write (`updateDrawingText`). Re-synced from the prop whenever
	// it changes out from under this bar: a commit round-tripping back
	// through the store, or the selection moving to a different drawing
	// (`draft` must not still show the previous note's text).
	let draft = $state(untrack(() => text ?? ''));
	$effect(() => {
		draft = text ?? '';
	});

	function commitText() {
		if (draft !== (text ?? '')) onChangeText(draft);
	}
</script>

<div
	class="absolute z-20 flex -translate-x-1/2 -translate-y-full items-center gap-2 border border-ink/18 bg-card2 px-2 py-[7px] shadow-[0_20px_48px_rgba(0,0,0,0.7)]"
	style={`left:${x}px; top:${y - 10}px;`}
>
	{#if kind === 'text_note'}
		<Input
			bind:value={draft}
			placeholder="Note text…"
			class="h-7 w-[150px] border-ink/14 bg-transparent font-mono text-[10.5px]"
			onblur={commitText}
			onkeydown={(e) => {
				if (e.key !== 'Enter') return;
				e.preventDefault();
				commitText();
				(e.currentTarget as HTMLInputElement).blur();
			}}
		/>
	{/if}

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

	<Tooltip.Provider>
		<Tooltip.Root>
			<Tooltip.Trigger
				class="flex cursor-pointer items-center border border-ink/14 px-[7px] py-[4px] text-dim2"
				onclick={onToggleVisible}
				aria-label={visible ? 'Hide drawing' : 'Show drawing'}
			>
				{#if visible}
					<EyeIcon class="size-3" />
				{:else}
					<EyeOffIcon class="size-3" />
				{/if}
			</Tooltip.Trigger>
			<Tooltip.Content side="top" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
				<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">{visible ? 'HIDE' : 'SHOW'}</span>
			</Tooltip.Content>
		</Tooltip.Root>

		<Tooltip.Root>
			<Tooltip.Trigger
				class="flex cursor-pointer items-center border border-ink/14 px-[7px] py-[4px] text-destructive"
				onclick={onDelete}
				aria-label="Delete drawing"
			>
				<Trash2Icon class="size-3" />
			</Tooltip.Trigger>
			<Tooltip.Content side="top" sideOffset={8} arrowClasses="hidden" class="rounded-none border border-ink/16 bg-popover px-2.5 py-1.5">
				<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">DELETE DRAWING</span>
			</Tooltip.Content>
		</Tooltip.Root>
	</Tooltip.Provider>
</div>
