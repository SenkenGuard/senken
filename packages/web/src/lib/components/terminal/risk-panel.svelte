<script lang="ts">
	// Risk meters widget body: a column of
	// `meter-bar` primitives, one per risk metric.
	import { MeterBar } from '$lib/components/ui/meter-bar/index.js';
	import type { RiskBar } from '$lib/mock/dashboard';

	let { bars }: { bars: RiskBar[] } = $props();

	const cls = {
		gain: { value: 'text-gain', bar: 'bg-gain' },
		loss: { value: 'text-loss', bar: 'bg-loss' },
		neutral: { value: 'text-foreground', bar: 'bg-foreground' }
	} as const;
</script>

<div class="flex h-full flex-col justify-between gap-[13px]">
	{#each bars as b (b.label)}
		<MeterBar
			label={b.label}
			value={b.value}
			fraction={b.fraction}
			cellCount={28}
			valueClass={cls[b.tone].value}
			barClass={cls[b.tone].bar}
		/>
	{/each}
</div>
