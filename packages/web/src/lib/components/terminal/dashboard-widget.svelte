<script lang="ts">
	// The 32px widget header + body frame every dashboard widget sits in: a diamond bullet, title, optional
	// meta text, and cycle/remove controls. Knows about the dashboard's
	// "span"/cycle/remove widget concept, so it lives in terminal/, not ui/.
	import { cn } from '$lib/utils.js';
	import MoveHorizontalIcon from '@lucide/svelte/icons/move-horizontal';
	import XIcon from '@lucide/svelte/icons/x';
	import type { Snippet } from 'svelte';

	let {
		title,
		meta = '',
		spanClass,
		minHeightClass,
		onCycle,
		onRemove,
		children
	}: {
		title: string;
		meta?: string;
		spanClass: string;
		minHeightClass: string;
		onCycle: () => void;
		onRemove: () => void;
		children: Snippet;
	} = $props();
</script>

<div class={cn('flex flex-col border border-border bg-card', spanClass, minHeightClass)}>
	<div class="flex h-8 flex-none items-center justify-between border-b border-ink/7 px-[11px]">
		<div class="flex min-w-0 items-center gap-2.5">
			<div class="size-[5px] flex-none rotate-45 bg-ink/30"></div>
			<span
				class="flex-none text-[10.5px] font-semibold tracking-[0.2em] whitespace-nowrap text-secondary-foreground uppercase"
			>
				{title}
			</span>
			{#if meta}
				<span class="min-w-0 flex-1 truncate font-mono text-[8.5px] tracking-[0.16em] text-dim">
					{meta}
				</span>
			{/if}
		</div>
		<div class="flex items-center gap-1">
			<button
				type="button"
				class="flex size-5 flex-none items-center justify-center text-dim"
				onclick={onCycle}
			>
				<MoveHorizontalIcon class="size-3" />
			</button>
			<button
				type="button"
				class="flex size-5 flex-none items-center justify-center text-dim"
				onclick={onRemove}
			>
				<XIcon class="size-3" />
			</button>
		</div>
	</div>
	<div class="min-h-0 flex-1 p-3">
		{@render children()}
	</div>
</div>
