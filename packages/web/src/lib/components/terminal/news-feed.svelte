<script lang="ts">
	// News tape widget body: a compact
	// headline stream with a time and a tag column.
	import { cn } from '$lib/utils.js';
	import type { FeedItem } from '$lib/mock/dashboard';

	let { items }: { items: FeedItem[] } = $props();

	const toneClass = { gain: 'text-gain', loss: 'text-loss', neutral: 'text-foreground' } as const;
</script>

<div class="flex h-full flex-col overflow-y-auto">
	{#each items as f, i (i)}
		<div class="flex gap-[11px] border-b border-ink/[0.045] py-2 last:border-0">
			<span class="w-[42px] flex-none font-mono text-[9px] text-dim">{f.time}</span>
			<span class={cn('w-[52px] flex-none font-mono text-[8.5px] tracking-[0.16em]', toneClass[f.tone])}>
				{f.tag}
			</span>
			<span class="text-[11px] leading-[1.45] text-pretty text-secondary-foreground">{f.text}</span>
		</div>
	{/each}
</div>
