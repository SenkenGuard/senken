<script lang="ts">
	// Signal desk widget body: a list of
	// model-verdict cards with a confidence bar.
	import { cn } from '$lib/utils.js';
	import type { Signal } from '$lib/mock/dashboard';

	let { signals }: { signals: Signal[] } = $props();

	const textClass = { gain: 'text-gain', loss: 'text-loss', neutral: 'text-foreground' } as const;
	const barClass = { gain: 'bg-gain', loss: 'bg-loss', neutral: 'bg-foreground' } as const;
</script>

<div class="flex h-full flex-col gap-[9px] overflow-y-auto">
	{#each signals as s (s.pair)}
		<div class="flex flex-col gap-[5px] border border-ink/8 bg-inv px-[11px] py-[9px]">
			<div class="flex items-center justify-between">
				<span class="font-mono text-[10.5px] tracking-[0.12em] text-foreground">{s.pair}</span>
				<span class={cn('font-mono text-[9px] tracking-[0.18em]', textClass[s.tone])}>{s.verdict}</span>
			</div>
			<div class="font-mono text-[9.5px] leading-[1.55] text-secondary-foreground">{s.note}</div>
			<div class="flex items-center gap-2">
				<div class="h-[3px] flex-1 bg-ink/8">
					<div class={cn('h-full', barClass[s.tone])} style={`width: ${s.conf};`}></div>
				</div>
				<span class="font-mono text-[8.5px] text-dim">{s.conf}</span>
			</div>
		</div>
	{/each}
</div>
