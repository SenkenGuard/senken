<script lang="ts">
	// Equity widget body: totalled equity + open PnL across every owned
	// account, then a per-account breakdown with balance, PnL and margin.
	//
	// No curve. Senken reads balances from an adapter on request and keeps no
	// equity history to plot one from — a sparkline here would have to be
	// invented, and this screen shows money. The curve returns once equity
	// snapshots are actually recorded somewhere.
	import { cn } from '$lib/utils.js';
	import * as Table from '$lib/components/ui/table/index.js';
	import type { DashboardEquityData, Tone } from '$lib/trade/view';

	let { data }: { data: DashboardEquityData } = $props();

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

<div class="flex h-full flex-col gap-2.5">
	{#if data.rows.length === 0}
		<div class="flex flex-1 items-center justify-center font-mono text-[10px] tracking-[0.18em] text-dim">
			NO ACCOUNTS ATTACHED
		</div>
	{:else}
		<div class="flex items-end gap-3.5">
			<div
				class={cn(
					'font-mono font-medium tracking-[-0.01em] text-foreground',
					data.totals?.mixedCurrency ? 'text-[16px]' : 'text-[30px]'
				)}
			>
				{data.totals?.equity ?? '—'}
			</div>
			{#if !data.totals?.mixedCurrency}
				<div class="flex flex-col gap-0.5 pb-[5px]">
					<span
						class={cn(
							'font-mono text-[11px]',
							data.totals ? toneClass[data.totals.pnlTone] : 'text-dim'
						)}
					>
						{data.totals?.pnl ?? '—'}
					</span>
					<span class="font-mono text-[8.5px] tracking-[0.18em] text-dim">OPEN PNL</span>
				</div>
			{/if}
		</div>

		{#if data.totals?.mixedCurrency}
			<!-- No single figure is honest across currencies with no FX rate
			     between them, so each currency's own subtotal is shown instead
			     of a blended total. -->
			<div class="flex flex-wrap gap-x-4 gap-y-1 border-b border-ink/8 pb-2.5" data-currency-subtotals>
				{#each data.totals.perCurrency as sub (sub.currency)}
					<div class="flex items-baseline gap-1.5">
						<span class="font-mono text-[11px] text-foreground">{sub.equity} {sub.currency}</span>
						<span class={cn('font-mono text-[9.5px]', toneClass[sub.pnlTone])}>{sub.pnl}</span>
					</div>
				{/each}
			</div>
		{/if}

		<div class="min-h-0 flex-1 overflow-auto border-t border-ink/8">
			<Table.Root>
				<Table.Header>
					<Table.Row class="border-ink/7 hover:bg-transparent">
						<Table.Head class="h-auto p-0 py-1.5 font-mono text-[8px] font-normal tracking-[0.2em] text-dim">
							ACCOUNT
						</Table.Head>
						<Table.Head
							class="h-auto p-0 py-1.5 text-right font-mono text-[8px] font-normal tracking-[0.2em] text-dim"
						>
							EQUITY
						</Table.Head>
						<Table.Head
							class="h-auto p-0 py-1.5 text-right font-mono text-[8px] font-normal tracking-[0.2em] text-dim"
						>
							PNL
						</Table.Head>
						<Table.Head
							class="h-auto p-0 py-1.5 text-right font-mono text-[8px] font-normal tracking-[0.2em] text-dim"
						>
							MARGIN
						</Table.Head>
					</Table.Row>
				</Table.Header>
				<Table.Body>
					{#each data.rows as row (row.id)}
						<Table.Row data-account-row={row.id} class="border-ink/[0.045] hover:bg-transparent">
							<Table.Cell class="p-0 py-1.5 font-mono text-[11px] text-foreground">{row.label}</Table.Cell>
							<Table.Cell class="p-0 py-1.5 text-right font-mono text-[11px] text-secondary-foreground">
								{row.equity}{row.currency ? ` ${row.currency}` : ''}
							</Table.Cell>
							<Table.Cell class={cn('p-0 py-1.5 text-right font-mono text-[11px]', toneClass[row.pnlTone])}>
								{row.pnl}
							</Table.Cell>
							<Table.Cell class="p-0 py-1.5 text-right font-mono text-[9.5px] tracking-[0.06em] text-dim2">
								{row.hasMargin ? `${row.marginUsed} / ${row.marginAvailable}` : 'NO MARGIN'}
							</Table.Cell>
						</Table.Row>
					{/each}
				</Table.Body>
			</Table.Root>
		</div>
	{/if}
</div>
