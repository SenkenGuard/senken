<script lang="ts">
	// Adapters view's card grid: adapter
	// header + status pill, supported-symbol chips, the accounts attached
	// through this adapter (each with a `live-dot` status indicator), and an
	// attach/connect footer row. Reuses `ui/card` and `ui/badge` per the reference;
	// `ui/live-dot` replaces the reference's raw 5px dot div.
	//
	// Correction to this file's earlier P4 comment: the
	// footer attach/connect row does *not* open the command palette in the
	// reference — `a.onAttach` (line 837) calls `attachAccount(a.key)`
	// directly (line 2958), same as `onSelectAccount` below. Only the page's
	// own top-bar "ATTACH ACCOUNT" button (`openAdapterCmd`, line 731) opens
	// the palette, in `'adapter'` mode, to *choose* which adapter to attach
	// next — this per-card button already knows which adapter it is.
	import { cn } from '$lib/utils.js';
	import * as Card from '$lib/components/ui/card/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import { LiveDot } from '$lib/components/ui/live-dot/index.js';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import type { AdapterCardData } from '$lib/trade/view';

	let {
		adapter,
		onSelectAccount,
		onAttach
	}: {
		adapter: AdapterCardData;
		onSelectAccount: (id: string) => void;
		onAttach: (adapterKey: string) => void;
	} = $props();
</script>

<Card.Root class="flex-col gap-0 rounded-none border-ink/9 bg-card p-0 shadow-none ring-0">
	<div class="flex items-start gap-[11px] border-b border-ink/6 px-3.5 py-[13px]">
		<adapter.icon class={cn('mt-px size-[18px] flex-none', adapter.connected ? 'text-foreground' : 'text-dim')} />
		<div class="flex min-w-0 flex-1 flex-col gap-[5px]">
			<div class="flex min-w-0 items-center gap-2">
				<span
					class="overflow-hidden text-ellipsis whitespace-nowrap text-[12.5px] font-medium tracking-[0.03em] text-foreground"
				>
					{adapter.name}
				</span>
				<Badge
					class={cn(
						'h-auto flex-none rounded-none px-[5px] py-[1px] font-mono text-[7.5px] tracking-[0.14em]',
						adapter.connected ? 'bg-gain text-inv' : 'bg-ink/10 text-dim2'
					)}
				>
					{adapter.status}
				</Badge>
			</div>
			<span class="font-mono text-[8.5px] tracking-[0.16em] text-dim">{adapter.kind} · {adapter.coverage}</span>
			<span class="font-mono text-[9.5px] leading-[1.55] text-dim2">{adapter.note}</span>
		</div>
	</div>

	<div class="flex flex-wrap gap-[5px] border-b border-ink/6 px-3.5 py-[10px]">
		{#each adapter.symbolChips as chip (chip)}
			<span class="border border-ink/10 px-[7px] py-[3px] font-mono text-[8.5px] tracking-[0.1em] text-dim2">
				{chip}
			</span>
		{/each}
	</div>

	{#each adapter.accounts as ac (ac.id)}
		<button
			type="button"
			onclick={() => onSelectAccount(ac.id)}
			class={cn(
				'flex items-center justify-between gap-2 border-b border-ink/5 px-3.5 py-[9px] text-left',
				ac.selected ? 'bg-ink/7' : 'bg-transparent'
			)}
		>
			<div class="flex min-w-0 items-center gap-2">
				<LiveDot class={ac.dotTone === 'loss' ? 'bg-loss' : 'bg-gain'} />
				<span
					class={cn(
						'overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[10px]',
						ac.selected ? 'text-foreground' : 'text-dim2'
					)}
				>
					{ac.label}
				</span>
			</div>
			<span class={cn('flex-none font-mono text-[9.5px]', ac.selected ? 'text-foreground' : 'text-dim2')}>
				{ac.equity}
			</span>
		</button>
	{/each}

	<button
		type="button"
		class="flex cursor-pointer items-center gap-2 px-3.5 py-[10px] text-left"
		onclick={() => onAttach(adapter.key)}
	>
		<PlusIcon class="size-[11px] text-dim" />
		<span class="font-mono text-[8.5px] tracking-[0.18em] text-dim">{adapter.attachLabel}</span>
	</button>
</Card.Root>
