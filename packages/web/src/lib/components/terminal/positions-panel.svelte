<script lang="ts">
	// Open positions widget body. Reuses
	// shadcn `table` per the reference ("positions" -> `table`), restyled to the
	// reference's compact density.
	import { cn } from '$lib/utils.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import type { PositionRow } from '$lib/mock/dashboard';

	let { rows }: { rows: PositionRow[] } = $props();

	const toneClass = { gain: 'text-gain', loss: 'text-loss', neutral: 'text-foreground' } as const;
</script>

<Table.Root>
	<Table.Header>
		<Table.Row class="border-ink/7 hover:bg-transparent">
			<Table.Head class="h-auto p-0 pb-1.5 font-mono text-[8px] font-normal tracking-[0.2em] text-dim">
				PAIR
			</Table.Head>
			<Table.Head class="h-auto p-0 pb-1.5 font-mono text-[8px] font-normal tracking-[0.2em] text-dim">
				SIDE
			</Table.Head>
			<Table.Head class="h-auto p-0 pb-1.5 text-right font-mono text-[8px] font-normal tracking-[0.2em] text-dim">
				ENTRY
			</Table.Head>
			<Table.Head class="h-auto p-0 pb-1.5 text-right font-mono text-[8px] font-normal tracking-[0.2em] text-dim">
				MARK
			</Table.Head>
			<Table.Head class="h-auto p-0 pb-1.5 text-right font-mono text-[8px] font-normal tracking-[0.2em] text-dim">
				PNL
			</Table.Head>
		</Table.Row>
	</Table.Header>
	<Table.Body>
		{#each rows as p (p.ticker)}
			<Table.Row class="border-ink/[0.045] hover:bg-transparent">
				<Table.Cell class="p-0 py-2 font-mono text-[11px] text-foreground">{p.ticker}</Table.Cell>
				<Table.Cell class={cn('p-0 py-2 font-mono text-[9px] tracking-[0.16em]', toneClass[p.sideTone])}>
					{p.side}
				</Table.Cell>
				<Table.Cell class="p-0 py-2 text-right font-mono text-[11px] text-secondary-foreground">
					{p.entry}
				</Table.Cell>
				<Table.Cell class="p-0 py-2 text-right font-mono text-[11px] text-secondary-foreground">
					{p.mark}
				</Table.Cell>
				<Table.Cell class={cn('p-0 py-2 text-right font-mono text-[11px]', toneClass[p.tone])}>
					{p.pnl}
				</Table.Cell>
			</Table.Row>
		{/each}
	</Table.Body>
</Table.Root>
