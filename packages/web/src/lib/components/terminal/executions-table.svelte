<script lang="ts">
	// The engine's POSITIONS / ORDERS / EXECUTIONS sub-tab bar and table. Reuses `ui/tabs` for the sub-bar
	// and `ui/table` for the grid, per the reference. The reference renders the
	// table as a CSS grid of divs with a per-tab `grid-template-columns`
	// string (`tableCols`) so it can vary column widths per tab without a
	// real `<table>`; here it's a semantic `<table>` instead (the reference: reuse
	// shadcn `table`), so those `fr` widths aren't reproduced literally —
	// columns size to content/available space instead of the reference's
	// exact proportions.
	//
	// The grid itself lives in `execution-grid.svelte` — see that file's own
	// docs for why it is a separate component rather than markup inlined
	// here.
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import type { EngineTable, EngineTableTab, TableTabDef } from '$lib/trade/view';
	import { cn } from '$lib/utils.js';
	import ExecutionGrid from './execution-grid.svelte';

	let {
		tabs,
		activeTab,
		ontabchange,
		table,
		onaction
	}: {
		tabs: TableTabDef[];
		activeTab: EngineTableTab;
		ontabchange: (tab: EngineTableTab) => void;
		table: EngineTable;
		/** Called with a row's own key (see `TableRow.key`) and the action's
		 * key (`'close' | 'cancel' | 'amend'`) when its button is clicked.
		 * Optional: a table with no actioned rows never needs it. */
		onaction?: (rowKey: string, actionKey: string) => void;
	} = $props();
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

	<ExecutionGrid {table} {onaction} />
</div>
