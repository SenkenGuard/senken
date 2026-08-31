<script lang="ts">
	// The engine's POSITIONS / ORDERS / EXECUTIONS sub-tab bar and table. Reuses `ui/tabs` for the sub-bar
	// and `ui/table` for the grid, per the reference. The reference renders the
	// table as a CSS grid of divs with a per-tab `grid-template-columns`
	// string (`tableCols`) so it can vary column widths per tab without a
	// real `<table>`; here it's a semantic `<table>` instead (the reference: reuse
	// shadcn `table`), so those `fr` widths aren't reproduced literally —
	// columns size to content/available space instead of the reference's
	// exact proportions.
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import { cn } from '$lib/utils.js';
	import type { EngineTable, EngineTableTab, TableTabDef, Tone } from '$lib/mock/engine';

	let {
		tabs,
		activeTab,
		ontabchange,
		table
	}: {
		tabs: TableTabDef[];
		activeTab: EngineTableTab;
		ontabchange: (tab: EngineTableTab) => void;
		table: EngineTable;
	} = $props();

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

<div class="flex min-h-0 flex-1 flex-col">
	<Tabs.Root value={activeTab} onValueChange={(v) => ontabchange(v as EngineTableTab)} class="contents">
		<Tabs.List
			class="group-data-horizontal/tabs:h-[34px] flex-none items-stretch justify-start gap-0 rounded-none bg-transparent p-0"
		>
			{#each tabs as t (t.key)}
				<Tabs.Trigger
					value={t.key}
					class="h-auto flex-none items-center gap-2 rounded-none border-0 border-r border-ink/6 px-4 py-0 font-mono text-[9.5px] tracking-[0.18em] text-dim2 uppercase shadow-none data-[state=active]:bg-foreground data-[state=active]:text-inv data-[state=active]:shadow-none dark:data-[state=active]:border-transparent dark:data-[state=active]:bg-foreground dark:data-[state=active]:text-inv"
				>
					{t.label}
					<span class={cn('font-mono text-[9px]', activeTab === t.key ? 'text-inv' : 'text-dim')}>
						{t.count}
					</span>
				</Tabs.Trigger>
			{/each}
		</Tabs.List>
	</Tabs.Root>

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
				</Table.Row>
			</Table.Header>
			<Table.Body>
				{#each table.rows as row, i (i)}
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
					</Table.Row>
				{/each}
			</Table.Body>
		</Table.Root>
		{#if table.rows.length === 0}
			<div class="py-[34px] text-center font-mono text-[10px] tracking-[0.18em] text-dim">
				{table.emptyLabel}
			</div>
		{/if}
	</div>
</div>
