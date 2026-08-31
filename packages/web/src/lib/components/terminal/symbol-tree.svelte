<script lang="ts">
	// Grouped, collapsible layer tree for the right panel's OBJECT TREE tab: instruments,
	// indicators, strategies and drawings for the active pane, each row with
	// a count header and a visibility toggle.
	import type { TreeGroup } from '$lib/charts/pane-runtime';
	import EyeIcon from '@lucide/svelte/icons/eye';
	import EyeOffIcon from '@lucide/svelte/icons/eye-off';

	let { groups }: { groups: TreeGroup[] } = $props();
</script>

<div>
	{#each groups as g (g.title)}
		<div class="border-b border-ink/6">
			<div class="flex items-center justify-between bg-ink/[0.03] px-3 py-1.5">
				<span class="font-mono text-[8px] tracking-[0.24em] text-dim">{g.title}</span>
				<span class="font-mono text-[9px] text-dim">{g.items.length}</span>
			</div>
			{#each g.items as item, i (item.name + i)}
				<div class="flex items-center justify-between gap-2 py-1.5 pr-3 pl-5">
					<div class="flex min-w-0 items-baseline gap-2">
						<span class={item.visible ? 'font-mono text-[10px] tracking-[0.08em] whitespace-nowrap text-secondary-foreground' : 'font-mono text-[10px] tracking-[0.08em] whitespace-nowrap text-dim'}>
							{item.name}
						</span>
						<span class="truncate font-mono text-[8.5px] text-dim">{item.params}</span>
					</div>
					{#if item.visible}
						<EyeIcon class="size-3 flex-none text-dim2" />
					{:else}
						<EyeOffIcon class="size-3 flex-none text-dim2" />
					{/if}
				</div>
			{/each}
		</div>
	{/each}
</div>
