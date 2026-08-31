<script lang="ts">
	// Watchlist widget body. Reuses
	// shadcn `table` per the reference ("positions" is mapped to `table`; the
	// watchlist is the same row shape), restyled to the reference's compact
	// density instead of the default padding/height.
	import { cn } from '$lib/utils.js';
	import { goto } from '$app/navigation';
	import * as Table from '$lib/components/ui/table/index.js';
	import type { WatchlistRow } from '$lib/mock/dashboard';

	let { rows }: { rows: WatchlistRow[] } = $props();

	const toneClass = { gain: 'text-gain', loss: 'text-loss', neutral: 'text-foreground' } as const;

	// reference: row `onClick` sets `page: 'charts', symbol: x.pair` — real
	// SvelteKit routing stands in for that state flip here.
	function open() {
		goto('/charts');
	}
</script>

<Table.Root>
	<Table.Header>
		<Table.Row class="border-ink/7 hover:bg-transparent">
			<Table.Head class="h-auto p-0 pb-1.5 font-mono text-[8px] font-normal tracking-[0.18em] text-dim">
				SYMBOL
			</Table.Head>
			<Table.Head class="h-auto p-0 pb-1.5 text-right font-mono text-[8px] font-normal tracking-[0.18em] text-dim">
				LAST
			</Table.Head>
			<Table.Head class="h-auto p-0 pb-1.5 text-right font-mono text-[8px] font-normal tracking-[0.18em] text-dim">
				24H
			</Table.Head>
		</Table.Row>
	</Table.Header>
	<Table.Body>
		{#each rows as r (r.ticker)}
			<Table.Row class="cursor-pointer border-ink/[0.045] hover:bg-ink/5" onclick={open}>
				<Table.Cell class="p-0 py-1.5">
					<div class="truncate font-mono text-[11px] font-medium tracking-[0.04em] text-foreground">
						{r.ticker}
					</div>
					<div class="truncate font-mono text-[8px] tracking-[0.14em] text-dim">{r.venue}</div>
				</Table.Cell>
				<Table.Cell class="p-0 py-1.5 text-right font-mono text-[10px] text-secondary-foreground">
					{r.last}
				</Table.Cell>
				<Table.Cell class={cn('p-0 py-1.5 text-right font-mono text-[10px]', toneClass[r.tone])}>
					{r.chg}
				</Table.Cell>
			</Table.Row>
		{/each}
	</Table.Body>
</Table.Root>
