<script lang="ts">
	// Equity widget body: headline value +
	// 24H delta, a sparkline against a benchmark, and three static
	// LONG/SHORT/CLOSE affordances. The reference itself does not bind
	// `onClick` on those three buttons (they are decorative in the design
	// canvas), so they stay inert here too.
	import { cn } from '$lib/utils.js';
	import { SparkLine } from '$lib/components/ui/spark-line/index.js';
	import type { EquityData } from '$lib/mock/dashboard';
	import TrendingUpIcon from '@lucide/svelte/icons/trending-up';
	import TrendingDownIcon from '@lucide/svelte/icons/trending-down';
	import CircleXIcon from '@lucide/svelte/icons/circle-x';

	let { equity }: { equity: EquityData } = $props();

	const toneClass = { gain: 'text-gain', loss: 'text-loss', neutral: 'text-foreground' } as const;

	const actions = [
		{ icon: TrendingUpIcon, label: 'LONG' },
		{ icon: TrendingDownIcon, label: 'SHORT' },
		{ icon: CircleXIcon, label: 'CLOSE' }
	];
</script>

<div class="flex h-full flex-col gap-2.5">
	<div class="flex items-end gap-3.5">
		<div class="font-mono text-[30px] font-medium tracking-[-0.01em] text-foreground">{equity.value}</div>
		<div class="flex flex-col gap-0.5 pb-[5px]">
			<span class={cn('font-mono text-[11px]', toneClass[equity.deltaTone])}>{equity.delta}</span>
			<span class="font-mono text-[8.5px] tracking-[0.18em] text-dim">24H NET</span>
		</div>
	</div>

	<div class="relative min-h-[70px] flex-1 border-b border-l border-ink/8">
		<SparkLine
			values={equity.series}
			benchmark={equity.benchmark}
			min={equity.min}
			max={equity.max}
			class="absolute inset-0 h-full w-full"
		/>
	</div>

	<div class="flex gap-1.5">
		{#each actions as a (a.label)}
			<div
				class="flex flex-1 cursor-pointer items-center justify-center gap-1.5 border border-ink/14 py-[7px] font-mono text-[9px] tracking-[0.18em] text-secondary-foreground"
			>
				<a.icon class="size-[13px]" />
				{a.label}
			</div>
		{/each}
	</div>
</div>
