<script lang="ts">
	// Generic coloured cell grid, used by the reference for the
	// volatility heatmap. Knows nothing
	// about volatility or time — just a list of 0-1 intensities laid out in a
	// fixed-column grid of square cells.
	import { cn } from '$lib/utils.js';

	let {
		cells,
		columns = 14,
		class: className,
		cellClass = 'bg-foreground'
	}: {
		/** One 0-1 opacity value per cell. */
		cells: number[];
		columns?: number;
		class?: string;
		cellClass?: string;
	} = $props();
</script>

<div
	data-slot="heat-grid"
	class={cn('grid gap-[3px]', className)}
	style={`grid-template-columns: repeat(${columns}, minmax(0, 1fr));`}
>
	{#each cells as opacity, i (i)}
		<div class={cn('aspect-square', cellClass)} style={`opacity: ${opacity};`}></div>
	{/each}
</div>
