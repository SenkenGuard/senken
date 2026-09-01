<script lang="ts">
	// Properties panel for a selected drawing object — the owner's own
	// acceptance point: "clicking an existing object selects it and offers
	// properties: colour, width, style," as TradingView does. Mirrors
	// layer-dialog.svelte's STYLE tab shell (same `ColorPicker`, the same
	// `LINE_STYLES`/width-button row), minus the tabs — a drawing has no
	// per-field plots or construction parameters, only the one colour/width/
	// line-style every drawing carries.
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { ColorPicker } from '$lib/components/ui/color-picker/index.js';
	import { cn } from '$lib/utils.js';
	import { LINE_STYLES, type LineStyle } from '$lib/mock/chart-settings';
	import type { DrawingRuntime } from '$lib/charts/pane-runtime';
	import MinusIcon from '@lucide/svelte/icons/minus';
	import SlashIcon from '@lucide/svelte/icons/slash';
	import RectangleHorizontalIcon from '@lucide/svelte/icons/rectangle-horizontal';
	import ArrowUpRightIcon from '@lucide/svelte/icons/arrow-up-right';
	import PercentIcon from '@lucide/svelte/icons/percent';
	import TypeIcon from '@lucide/svelte/icons/type';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import XIcon from '@lucide/svelte/icons/x';

	let {
		drawing,
		onClose,
		onChangeStyle,
		onDelete
	}: {
		drawing: DrawingRuntime | null;
		onClose: () => void;
		onChangeStyle: (patch: Partial<Pick<DrawingRuntime, 'color' | 'width' | 'lineStyle'>>) => void;
		onDelete: () => void;
	} = $props();

	const ICON = {
		horizontal_line: MinusIcon,
		trend_line: SlashIcon,
		rectangle: RectangleHorizontalIcon,
		ray: ArrowUpRightIcon,
		fib_retracement: PercentIcon,
		text_note: TypeIcon
	} as const;
	const TITLE = {
		horizontal_line: 'HORIZONTAL LINE',
		trend_line: 'TREND LINE',
		rectangle: 'RECTANGLE',
		ray: 'RAY',
		fib_retracement: 'FIB RETRACEMENT',
		text_note: 'TEXT NOTE'
	} as const;

	const Icon = $derived(drawing ? ICON[drawing.kind] : null);
	const title = $derived(drawing ? TITLE[drawing.kind] : '');

	function pickStyle(style: LineStyle) {
		onChangeStyle({ lineStyle: style });
	}

	function pickWidth(width: number) {
		onChangeStyle({ width });
	}

	function del() {
		onDelete();
		onClose();
	}
</script>

<Dialog.Root
	open={!!drawing}
	onOpenChange={(v) => {
		if (!v) onClose();
	}}
>
	<Dialog.Content
		showCloseButton={false}
		class="top-[74px] flex w-[340px] max-w-[92%] translate-y-0 flex-col gap-0 overflow-hidden border-ink/18 bg-card2 p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		{#if drawing}
			<Dialog.Header class="flex-row items-center justify-between gap-3 space-y-0 px-4 py-[13px]">
				<div class="flex min-w-0 items-center gap-[9px]">
					{#if Icon}<Icon class="size-[15px] flex-none text-dim2" />{/if}
					<Dialog.Title class="truncate text-[13px] font-medium tracking-[0.03em] text-foreground">{title}</Dialog.Title>
				</div>
				<Dialog.Close class="flex-none cursor-pointer text-dim2" aria-label="Close">
					<XIcon class="size-[14px]" />
				</Dialog.Close>
			</Dialog.Header>
			<Dialog.Description class="sr-only">Edit {title} properties</Dialog.Description>

			<div class="flex flex-col gap-[11px] px-4 pt-[14px] pb-4">
				<div class="flex items-center gap-2.5">
					<span class="flex-none basis-[74px] font-mono text-[8.5px] tracking-[0.16em] text-dim">COLOR</span>
					<ColorPicker value={drawing.color} fallback="#f2f2ef" onChange={(hex) => onChangeStyle({ color: hex })} />
				</div>
				<div class="flex items-center gap-2.5">
					<span class="flex-none basis-[74px] font-mono text-[8.5px] tracking-[0.16em] text-dim">LINE</span>
					<div class="flex gap-[5px]">
						{#each LINE_STYLES as s (s)}
							<button
								type="button"
								class={cn('cursor-pointer border px-[9px] py-1', drawing.lineStyle === s ? 'border-foreground bg-foreground' : 'border-ink/14')}
								onclick={() => pickStyle(s)}
							>
								<span class={cn('font-mono text-[8.5px] tracking-[0.14em]', drawing.lineStyle === s ? 'text-inv' : 'text-dim2')}>{s}</span>
							</button>
						{/each}
					</div>
				</div>
				<div class="flex items-center gap-2.5">
					<span class="flex-none basis-[74px] font-mono text-[8.5px] tracking-[0.16em] text-dim">WIDTH</span>
					<div class="flex gap-[5px]">
						{#each [1, 2, 3, 4] as w (w)}
							<button
								type="button"
								class={cn('w-[26px] cursor-pointer border py-1 text-center', drawing.width === w ? 'border-foreground bg-foreground' : 'border-ink/14')}
								onclick={() => pickWidth(w)}
							>
								<span class={cn('font-mono text-[8.5px]', drawing.width === w ? 'text-inv' : 'text-dim2')}>{w}</span>
							</button>
						{/each}
					</div>
				</div>
			</div>

			<div class="flex flex-none items-center justify-between gap-[7px] border-t border-ink/10 px-4 py-[11px]">
				<button type="button" class="flex cursor-pointer items-center gap-1.5 border border-ink/14 px-3 py-[7px] text-destructive" onclick={del}>
					<Trash2Icon class="size-3" />
					<span class="font-mono text-[9px] tracking-[0.18em]">DELETE</span>
				</button>
				<button type="button" class="cursor-pointer bg-foreground px-[18px] py-[7px]" onclick={onClose}>
					<span class="font-mono text-[9px] font-semibold tracking-[0.18em] text-inv">OK</span>
				</button>
			</div>
		{/if}
	</Dialog.Content>
</Dialog.Root>
