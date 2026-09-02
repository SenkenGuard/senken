<script lang="ts">
	// The horizontal stat-cell strip used by both the Overview view's
	// aggStats row and the
	// Accounts view's acctStats row (lines 797-805, `size="md"`) — same
	// shape, different padding/type scale in the reference, so one
	// component with a size prop instead of two near-duplicates.
	//
	// Not built on `ui/stat-cell`: that primitive's label/value sizes are
	// fixed (7.5px / text-xs) for the TopBar/FooterBar HUD it was pulled
	// forward for in P1, and this strip needs the engine page's own sizes
	// (8px label, 14-15px value) that stat-cell doesn't expose as a prop —
	// see the P4 implementation report for the full reasoning.
	import { cn } from '$lib/utils.js';
	import type { StatItem, Tone } from '$lib/trade/view';

	let { stats, size = 'lg' }: { stats: StatItem[]; size?: 'lg' | 'md' } = $props();

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

<div class="flex flex-none items-stretch border-b border-ink/9 bg-bg2">
	{#each stats as st (st.label)}
		<div
			class={cn(
				'flex min-w-0 flex-1 flex-col gap-1 overflow-hidden border-r border-ink/6',
				size === 'lg' ? 'px-[15px] py-[13px]' : 'px-3.5 py-3'
			)}
		>
			<span
				class="overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[8px] tracking-[0.18em] text-dim"
			>
				{st.label}
			</span>
			<span
				class={cn(
					'overflow-hidden text-ellipsis whitespace-nowrap font-mono',
					size === 'lg' ? 'text-[15px]' : 'text-[14px]',
					toneClass[st.tone]
				)}
			>
				{st.value}
			</span>
			{#if st.detail}
				<div class="flex flex-col gap-px" data-stat-detail>
					{#each st.detail as line (line)}
						<span class="overflow-hidden text-ellipsis whitespace-nowrap font-mono text-[9px] text-dim2">
							{line}
						</span>
					{/each}
				</div>
			{/if}
		</div>
	{/each}
</div>
