<script lang="ts">
	// Volatility heatmap widget body: a
	// `heat-grid` primitive plus the MAY/JUN/JUL/AUG legend row.
	import { HeatGrid } from '$lib/components/ui/heat-grid/index.js';
	import { HEAT_MONTH_LABELS } from '$lib/mock/dashboard';

	let { cells }: { cells: number[] } = $props();
</script>

<div class="flex h-full flex-col gap-[9px]">
	<HeatGrid {cells} columns={14} cellClass="bg-foreground" class="flex-1" />
	<div class="flex justify-between font-mono text-[8px] tracking-[0.16em] text-dim">
		{#each HEAT_MONTH_LABELS as m (m)}
			<span>{m}</span>
		{/each}
	</div>
</div>
