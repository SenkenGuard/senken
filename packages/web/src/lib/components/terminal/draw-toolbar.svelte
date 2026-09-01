<script lang="ts">
	// Vertical drawing-tool strip: the
	// DRAW_TOOLS list plus a pinned eraser at the bottom. Follows the same
	// plain-button + Tooltip pattern nav-rail.svelte already established for
	// this codebase's other icon-only vertical rails, rather than shadcn's
	// Tabs (the reference lumps this under "tabs", but Tabs is a content pager — an
	// icon-only rail with no associated panel content is what nav-rail
	// already solved with Tooltip.Root + plain buttons).
	import { cn } from '$lib/utils.js';
	import { DRAW_TOOLS, type ToolKey } from './chart-config';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import EraserIcon from '@lucide/svelte/icons/eraser';

	let {
		tool,
		onPick,
		onClear
	}: {
		tool: ToolKey;
		onPick: (key: ToolKey) => void;
		onClear: () => void;
	} = $props();

	const flyoutClass = 'rounded-none border border-ink/16 bg-popover px-2.5 py-1.5 text-left shadow-[0_12px_28px_rgba(0,0,0,0.6)]';
</script>

<Tooltip.Provider>
	<div
		class="flex w-[42px] flex-none flex-col items-center gap-0.5 border-r border-border bg-chrome py-1.5"
	>
		{#each DRAW_TOOLS as t (t.key)}
			{@const active = tool === t.key}
			<Tooltip.Root>
				<Tooltip.Trigger
					class={cn(
						'flex h-7 w-full max-w-[30px] cursor-pointer items-center justify-center border transition-colors',
						active
							? 'border-foreground bg-foreground text-inv'
							: 'border-transparent text-dim2 hover:bg-ink/7 hover:text-foreground'
					)}
					onclick={() => onPick(t.key)}
					aria-label={t.label}
				>
					<t.icon class="size-[15px]" />
				</Tooltip.Trigger>
				<Tooltip.Content side="right" sideOffset={10} arrowClasses="hidden" class={flyoutClass}>
					<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">
						{t.label}
					</span>
				</Tooltip.Content>
			</Tooltip.Root>
		{/each}

		<div class="flex-1"></div>

		<Tooltip.Root>
			<Tooltip.Trigger
				class="flex h-7 w-full max-w-[30px] cursor-pointer items-center justify-center text-dim"
				onclick={onClear}
				aria-label="Clear this pane's drawings"
			>
				<EraserIcon class="size-[15px]" />
			</Tooltip.Trigger>
			<Tooltip.Content side="right" sideOffset={10} arrowClasses="hidden" class={flyoutClass}>
				<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">
					CLEAR PANE DRAWINGS
				</span>
			</Tooltip.Content>
		</Tooltip.Root>
	</div>
</Tooltip.Provider>
