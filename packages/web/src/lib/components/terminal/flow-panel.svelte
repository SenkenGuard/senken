<script lang="ts">
	// Buy/sell flow widget body: a stat
	// row (net buy / txns / vol) over a paired up/down bar chart.
	import { FLOW_SUMMARY, type FlowBar } from '$lib/mock/dashboard';

	let { bars }: { bars: FlowBar[] } = $props();
</script>

<div class="flex h-full flex-col gap-2">
	<div class="flex gap-4">
		<div class="flex flex-col gap-0.5">
			<span class="font-mono text-[8px] tracking-[0.2em] text-dim">NET BUY</span>
			<span class="font-mono text-[15px] text-gain">{FLOW_SUMMARY.netBuy}</span>
		</div>
		<div class="flex flex-col gap-0.5">
			<span class="font-mono text-[8px] tracking-[0.2em] text-dim">TXNS</span>
			<span class="font-mono text-[15px] text-secondary-foreground">{FLOW_SUMMARY.txns}</span>
		</div>
		<div class="flex flex-col gap-0.5">
			<span class="font-mono text-[8px] tracking-[0.2em] text-dim">VOL</span>
			<span class="font-mono text-[15px] text-secondary-foreground">{FLOW_SUMMARY.vol}</span>
		</div>
	</div>
	<div class="flex h-24 items-end gap-[7px] pt-1.5">
		{#each bars as f, i (i)}
			<div class="flex h-full flex-1 items-end gap-0.5">
				<div class="flex-1 bg-secondary-foreground" style={`height: ${f.up};`}></div>
				<div class="flex-1 bg-ink/20" style={`height: ${f.down};`}></div>
			</div>
		{/each}
	</div>
</div>
