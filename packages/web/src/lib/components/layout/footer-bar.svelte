<script lang="ts">
	// 30px footer bar: compact status
	// segments with detail cards, the market-tape marquee, and a widget
	// picker mirroring TopBar's.
	//
	// the reference opens these detail cards on hover
	// (`onMouseEnter`/`onMouseLeave`) via `HoverCard`, but the product owner
	// found that too eager for a bar this dense with adjacent targets —
	// changed to open on click, like every other popover in this shell
	// (workspace menu, nav/footer widget pickers). `openKey` tracks which
	// segment's card is open (at most one at a time, `Popover` semantics),
	// clicking the same segment again or clicking outside closes it.
	import { cn } from '$lib/utils.js';
	import { FOOT_WIDGETS, DEFAULT_FOOT_KEYS, TICKER_ITEMS } from '$lib/mock/shell';
	import { TickerTape } from '$lib/components/ui/ticker-tape/index.js';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import SlidersHorizontalIcon from '@lucide/svelte/icons/sliders-horizontal';

	let openKey = $state<string | null>(null);
	let footPickerOpen = $state(false);
	let visibleFootKeys = $state<string[]>([...DEFAULT_FOOT_KEYS]);
	const visibleSegs = $derived(FOOT_WIDGETS.filter((w) => visibleFootKeys.includes(w.key)));

	function toggleFoot(key: string) {
		visibleFootKeys = visibleFootKeys.includes(key)
			? visibleFootKeys.filter((k) => k !== key)
			: [...visibleFootKeys, key];
	}

	const toneClass = { gain: 'text-gain', loss: 'text-loss', neutral: 'text-secondary-foreground' } as const;
</script>

<footer class="relative flex h-[30px] flex-none items-stretch overflow-hidden border-t border-border bg-chrome">
	<div class="flex min-w-0 flex-1 items-stretch overflow-hidden">
		{#each visibleSegs as w (w.key)}
			{#if w.key === 'ticker'}
				<div class="flex min-w-[120px] flex-1 items-stretch gap-2.5 border-r border-ink/6 px-3.5">
					<TickerTape items={TICKER_ITEMS} durationSeconds={46}>
						{#snippet item(t)}
							<div class="flex items-center gap-2.5 font-mono text-[10.5px]">
								<span class="tracking-[0.14em] text-secondary-foreground">{t.pair}</span>
								<span class="text-foreground">{t.last}</span>
								<span class={t.positive ? 'text-gain' : 'text-loss'}>{t.chg}</span>
							</div>
						{/snippet}
					</TickerTape>
				</div>
			{:else}
				<div class="flex flex-none items-center gap-2.5 border-r border-ink/6 px-3.5">
					<Popover.Root
						open={openKey === w.key}
						onOpenChange={(v) => (openKey = v ? w.key : null)}
					>
						<Popover.Trigger>
							{#snippet child({ props })}
								<div {...props} class="flex cursor-pointer items-center gap-1.5 whitespace-nowrap">
									<w.icon class={cn('size-3', w.key === 'status' ? 'text-gain' : 'text-dim2')} />
									<span class="font-mono text-[9px] tracking-[0.16em] text-dim2">{w.short}</span>
								</div>
							{/snippet}
						</Popover.Trigger>
						<Popover.Content
							side="top"
							align="start"
							sideOffset={12}
							class="w-[232px] rounded-none border border-ink/18 bg-popover p-0 shadow-[0_-18px_44px_rgba(0,0,0,0.7)]"
						>
							<div class="border-b border-ink/7 px-3 py-2 font-mono text-[8px] tracking-[0.24em] text-dim">
								{w.label}
							</div>
							{#each w.details as d (d.k)}
								<div class="flex items-center justify-between gap-3.5 border-b border-ink/[4.5%] px-3 py-1.5">
									<span class="font-mono text-[9px] tracking-[0.14em] text-secondary-foreground">{d.k}</span>
									<span class={cn('font-mono text-[9.5px] whitespace-nowrap', toneClass[d.tone])}>{d.v}</span>
								</div>
							{/each}
						</Popover.Content>
					</Popover.Root>
				</div>
			{/if}
		{/each}
	</div>

	<Popover.Root bind:open={footPickerOpen}>
		<Popover.Trigger
			class={cn(
				'flex flex-none cursor-pointer items-center border-l border-ink/8 px-2.5 transition-colors',
				footPickerOpen ? 'bg-foreground text-inv' : 'text-dim2'
			)}
		>
			<SlidersHorizontalIcon class="size-[13px]" />
		</Popover.Trigger>
		<Popover.Content align="end" side="top" sideOffset={12} class="w-[226px] border-ink/18 bg-popover p-0">
			<div class="border-b border-ink/7 px-3 py-2.5 font-mono text-[8px] tracking-[0.24em] text-dim">
				FOOTER BAR WIDGETS
			</div>
			{#each FOOT_WIDGETS as w (w.key)}
				{@const on = visibleFootKeys.includes(w.key)}
				<div class="flex items-center justify-between border-b border-ink/[4.5%] px-3 py-2">
					<span class={cn('font-mono text-[10px] tracking-[0.14em]', on ? 'text-foreground' : 'text-dim2')}>
						{w.label}
					</span>
					<Switch size="sm" checked={on} onCheckedChange={() => toggleFoot(w.key)} />
				</div>
			{/each}
		</Popover.Content>
	</Popover.Root>
</footer>
