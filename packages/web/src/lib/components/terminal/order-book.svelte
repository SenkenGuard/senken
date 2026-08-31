<script lang="ts">
	// Depth ladder with proportional background fills: asks (loss-tinted) stacked above bids (gain-tinted),
	// each row's fill width driven by `depthPct`.
	import { cn } from '$lib/utils.js';
	import type { BookRow } from './trade-demo';

	let { rows }: { rows: BookRow[] } = $props();
</script>

<div>
	{#each rows as row, i (i)}
		<div class="relative flex items-center justify-between px-3 py-1 font-mono text-[10px]">
			<div
				class={cn('absolute inset-y-0 right-0', row.side === 'ask' ? 'bg-loss/10' : 'bg-gain/10')}
				style="width: {row.depthPct}%;"
			></div>
			<span class={cn('relative', row.side === 'ask' ? 'text-loss' : 'text-gain')}>{row.price}</span>
			<span class="relative text-secondary-foreground">{row.size}</span>
		</div>
	{/each}
</div>
