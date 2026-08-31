<script lang="ts">
	// The right-click menu for a chart pane. Two shapes from one component,
	// because they share every mechanic except their items: the plot area's
	// menu and the price scale's menu. Positioned by the caller in viewport
	// coordinates and clamped here so a right-click near an edge does not
	// open a menu half off-screen.
	import CheckIcon from '@lucide/svelte/icons/check';

	export type MenuItem =
		| { kind: 'action'; label: string; hint?: string; onSelect: () => void }
		| { kind: 'toggle'; label: string; checked: boolean; onSelect: () => void }
		| { kind: 'separator' };

	let {
		x,
		y,
		items,
		onClose
	}: {
		x: number;
		y: number;
		items: MenuItem[];
		onClose: () => void;
	} = $props();

	const WIDTH = 232;
	const ITEM_H = 26;
	const SEPARATOR_H = 9;

	const height = $derived(
		items.reduce((sum, i) => sum + (i.kind === 'separator' ? SEPARATOR_H : ITEM_H), 8)
	);
	const left = $derived(Math.min(x, (globalThis.innerWidth ?? 0) - WIDTH - 8));
	const top = $derived(Math.min(y, (globalThis.innerHeight ?? 0) - height - 8));
</script>

<svelte:window
	onkeydown={(e) => e.key === 'Escape' && onClose()}
	onresize={onClose}
/>

<!-- The backdrop closes the menu on any click, including a second
     right-click somewhere else on the chart. Escape closes it too, via the
     window handler above, so this carries no keyboard role of its own. -->
<div
	class="fixed inset-0 z-50"
	role="presentation"
	onclick={onClose}
	oncontextmenu={(e) => {
		e.preventDefault();
		onClose();
	}}
>
	<div
		role="menu"
		tabindex="-1"
		class="absolute border border-ink/16 bg-popover py-1 shadow-lg"
		style="left:{left}px; top:{top}px; width:{WIDTH}px;"
		onclick={(e) => e.stopPropagation()}
		onkeydown={() => {}}
	>
		{#each items as item, i (i)}
			{#if item.kind === 'separator'}
				<div class="my-1 border-t border-ink/12"></div>
			{:else}
				<button
					type="button"
					role="menuitem"
					class="flex h-[26px] w-full cursor-pointer items-center gap-2 px-2.5 text-left hover:bg-ink/8"
					onclick={() => {
						item.onSelect();
						onClose();
					}}
				>
					<span class="flex w-3 flex-none justify-center">
						{#if item.kind === 'toggle' && item.checked}
							<CheckIcon class="size-[11px] text-foreground" />
						{/if}
					</span>
					<span class="flex-1 font-mono text-[10px] tracking-[0.1em] text-secondary-foreground">
						{item.label}
					</span>
					{#if item.kind === 'action' && item.hint}
						<span class="font-mono text-[9px] tracking-[0.1em] text-dim">{item.hint}</span>
					{/if}
				</button>
			{/if}
		{/each}
	</div>
</div>
