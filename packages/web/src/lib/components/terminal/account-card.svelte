<script lang="ts">
	// Accounts view's card strip. Reuses
	// `ui/card` (the reference: "Account avatars" -> `avatar`, but the card shape
	// itself maps to `card`) and `ui/badge` for the DEMO/LIVE mode pill,
	// restyled through tokens per Part its rule.
	import { cn } from '$lib/utils.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import type { AccountCardData } from '$lib/mock/engine';

	let { card, onclick }: { card: AccountCardData; onclick: () => void } = $props();
</script>

<Card.Root
	role="button"
	tabindex={0}
	{onclick}
	onkeydown={(e) => {
		if (e.key === 'Enter' || e.key === ' ') {
			e.preventDefault();
			onclick();
		}
	}}
	class={cn(
		'w-[214px] flex-none cursor-pointer flex-col gap-[7px] rounded-none border p-3 shadow-none ring-0',
		card.selected ? 'border-foreground bg-ink/6' : 'border-ink/10 bg-transparent'
	)}
>
	<div class="flex min-w-0 items-center gap-2">
		<card.icon class="size-[13px] flex-none text-dim2" />
		<span
			class={cn(
				'overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[10px] tracking-[0.04em]',
				card.selected ? 'text-foreground' : 'text-secondary-foreground'
			)}
		>
			{card.label}
		</span>
		<Badge
			class={cn(
				'h-auto flex-none rounded-none px-[4px] py-[1px] font-mono text-[7.5px] tracking-[0.14em]',
				card.mode === 'LIVE' ? 'bg-loss text-inv' : 'bg-ink/12 text-dim2'
			)}
		>
			{card.mode}
		</Badge>
	</div>
	<span class="font-mono text-[15px] text-foreground">{card.equity}</span>
	<div class="flex items-center justify-between">
		<span class="font-mono text-[8.5px] tracking-[0.14em] text-dim">{card.adapterName}</span>
		<span class={cn('font-mono text-[9.5px]', card.pnlTone === 'gain' ? 'text-gain' : 'text-loss')}>
			{card.pnl}
		</span>
	</div>
</Card.Root>
