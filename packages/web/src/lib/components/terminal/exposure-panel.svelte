<script lang="ts">
	// Overview view's "EQUITY BY ADAPTER" panel: a header line with the account/adapter counts, then
	// one proportional bar per adapter the account set touches.
	import { cn } from '$lib/utils.js';
	import type { ExposureItem } from '$lib/trade/view';

	let {
		items,
		accounts,
		adapters
	}: { items: ExposureItem[]; accounts: number; adapters: number } = $props();
</script>

<div class="flex-none border-b border-ink/9 bg-background px-4 pt-[13px] pb-[15px]">
	<div class="mb-[11px] flex items-center justify-between">
		<span class="font-mono text-[8px] tracking-[0.24em] text-dim">EQUITY BY ADAPTER</span>
		<span class="font-mono text-[8px] tracking-[0.18em] text-dim">
			{accounts} ACCOUNTS · {adapters} ADAPTERS
		</span>
	</div>
	<div class="flex flex-col gap-[9px]">
		{#each items as x (x.name)}
			<div class="flex items-center gap-3">
				<x.icon class="size-3.5 flex-none text-dim2" />
				<span
					class="w-[132px] flex-none overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[10px] tracking-[0.04em] text-secondary-foreground"
				>
					{x.name}
				</span>
				<div class="h-1.5 min-w-0 flex-1 bg-ink/7">
					<div class={cn('h-full', x.highlighted ? 'bg-foreground' : 'bg-ink/45')} style={`width: ${x.pct}%`}
					></div>
				</div>
				<span class="w-[42px] flex-none text-right font-mono text-[9.5px] text-dim2">{x.pct}%</span>
				<span class="w-28 flex-none text-right font-mono text-[10.5px] text-foreground">
					{x.equity}
					<span class="text-dim2">{x.currency}</span>
				</span>
			</div>
		{/each}
	</div>
</div>
