<script lang="ts">
	// The POSITIONS / ORDERS / EXECUTIONS grid itself, split out of
	// `executions-table.svelte` so it can be rendered — and its trailing
	// actions column tested — without that component's `Tabs.Root`: bits-ui's
	// roving-focus machinery touches browser-only state at module load and
	// cannot be compiled through this project's server-render test harness
	// (see `scripts/svelte-server-preload.ts`'s own docs on what it does and
	// does not cover). This component has no such dependency: props in,
	// markup out.
	import * as Table from '$lib/components/ui/table/index.js';
	import { cn } from '$lib/utils.js';
	import type { EngineTable, Tone } from '$lib/trade/view';

	let {
		table,
		onaction
	}: {
		table: EngineTable;
		/** Called with a row's own key (see `TableRow.key`) and the action's
		 * key (`'close' | 'cancel' | 'amend'`) when its button is clicked.
		 * Optional: a table with no actioned rows never needs it. */
		onaction?: (rowKey: string, actionKey: string) => void;
	} = $props();

	// The trailing column only exists when at least one row actually has
	// something to put in it — a table full of read-only rows must not grow
	// an empty column nobody can use.
	const hasActions = $derived(table.rows.some((row) => (row.actions?.length ?? 0) > 0));

	const toneClass: Record<Tone, string> = {
		fg: 'text-foreground',
		fg2: 'text-secondary-foreground',
		dim: 'text-dim',
		dim2: 'text-dim2',
		gain: 'text-gain',
		loss: 'text-loss',
		inv: 'text-inv'
	};
</script>

<div class="min-h-0 flex-1 overflow-auto bg-background">
	<Table.Root>
		<Table.Header class="sticky top-0 z-10 bg-bg2">
			<Table.Row class="border-ink/8 hover:bg-transparent">
				{#each table.columns as col (col.label)}
					<Table.Head
						class={cn(
							'h-auto overflow-hidden px-3 py-[9px] text-ellipsis whitespace-nowrap font-mono text-[8px] font-normal tracking-[0.18em] text-dim uppercase',
							col.align === 'right' ? 'text-right' : 'text-left'
						)}
					>
						{col.label}
					</Table.Head>
				{/each}
				{#if hasActions}
					<Table.Head
						class="h-auto overflow-hidden px-3 py-[9px] text-right font-mono text-[8px] font-normal tracking-[0.18em] text-dim uppercase"
					></Table.Head>
				{/if}
			</Table.Row>
		</Table.Header>
		<Table.Body>
			{#each table.rows as row (row.key)}
				<Table.Row class="border-ink/5 hover:bg-transparent">
					{#each row.cells as cell, j (j)}
						<Table.Cell
							class={cn(
								'overflow-hidden px-3 py-[11px] text-ellipsis whitespace-nowrap font-mono text-[11px] tracking-[0.03em]',
								cell.align === 'right' ? 'text-right' : 'text-left',
								toneClass[cell.tone]
							)}
						>
							{cell.value}
						</Table.Cell>
					{/each}
					{#if hasActions}
						<Table.Cell class="overflow-hidden px-3 py-[11px] text-right whitespace-nowrap">
							<div class="flex justify-end gap-2.5">
								{#each row.actions ?? [] as action (action.key)}
									<button
										type="button"
										class={cn(
											'cursor-pointer font-mono text-[9px] tracking-[0.14em]',
											action.tone === 'destructive'
												? 'text-dim hover:text-loss'
												: 'text-dim hover:text-foreground'
										)}
										onclick={() => onaction?.(row.key, action.key)}
									>
										{action.label}
									</button>
								{/each}
							</div>
						</Table.Cell>
					{/if}
				</Table.Row>
			{/each}
		</Table.Body>
	</Table.Root>
	{#if table.rows.length === 0}
		<div class="py-[34px] text-center font-mono text-[10px] tracking-[0.18em] text-dim">
			{table.loading ? 'LOADING…' : table.emptyLabel}
		</div>
	{/if}
</div>
