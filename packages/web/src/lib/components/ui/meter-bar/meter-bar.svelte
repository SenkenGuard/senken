<script lang="ts">
	// Generic labelled horizontal meter: a label/value header over a
	// row of discrete filled cells, used by the reference for risk and
	// exposure gauges. Knows nothing about
	// risk, portfolios, or any other app concept — just a label, a value
	// string, and a fill fraction.
	import { cn } from '$lib/utils.js';

	let {
		label,
		value,
		fraction,
		cellCount = 28,
		class: className,
		labelClass = 'text-secondary-foreground',
		valueClass = 'text-foreground',
		barClass = 'bg-foreground'
	}: {
		label: string;
		value: string;
		/** 0-1 fill fraction. Clamped before use. */
		fraction: number;
		cellCount?: number;
		class?: string;
		labelClass?: string;
		valueClass?: string;
		barClass?: string;
	} = $props();

	const filled = $derived(Math.round(Math.min(1, Math.max(0, fraction)) * cellCount));
</script>

<div data-slot="meter-bar" class={cn('flex flex-col gap-1.5', className)}>
	<div class="flex items-baseline justify-between">
		<span class={cn('font-mono text-[8.5px] tracking-[0.22em]', labelClass)}>{label}</span>
		<span class={cn('font-mono text-[11px]', valueClass)}>{value}</span>
	</div>
	<div class="flex h-1.5 gap-0.5 overflow-hidden">
		{#each Array(cellCount) as _, i (i)}
			<div class={cn('flex-1', i < filled ? barClass : 'bg-ink/8')}></div>
		{/each}
	</div>
</div>
