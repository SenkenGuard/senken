<script lang="ts">
	// 46px shared chrome header: wordmark,
	// the HUD stat strip, the nav-widget picker, a live clock and the account
	// avatar. Visually distinct from NavRail, which is the 52px sidebar below
	// it (lines 76-101) — the reference nests both under one flex column, but
	// they are two different pieces of chrome and two different components
	// here.
	import { cn } from '$lib/utils.js';
	import { tradeStore } from '$lib/state/trade.svelte';
	import { DEFAULT_HUD_KEYS, hudStats, type Tone } from '$lib/trade/view';
	import { StatCell } from '$lib/components/ui/stat-cell/index.js';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import * as Avatar from '$lib/components/ui/avatar/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import ConnectionStatus from './connection-status.svelte';
	import SlidersHorizontalIcon from '@lucide/svelte/icons/sliders-horizontal';

	let navPickerOpen = $state(false);
	let visibleHudKeys = $state<string[]>([...DEFAULT_HUD_KEYS]);
	// Read out of the shared trade store on every run, never captured once:
	// this bar sits above every route, so it must show what the store holds
	// now — including a fill that happened on a screen the user is not
	// looking at. It starts no polling of its own; whichever route is
	// mounted decides through `watchTrade` what is kept fresh, and this
	// simply renders whatever that leaves in the store.
	const hud = $derived(hudStats(tradeStore.accounts, tradeStore.portfolios));
	const visibleHud = $derived(hud.filter((w) => visibleHudKeys.includes(w.key)));

	/** `StatCell` has four tones; the trade view models have seven. The two
	 * that carry meaning — gain and loss — map across unchanged, and the
	 * rest collapse to prominent or muted. */
	const CELL_TONE: Record<Tone, 'bright' | 'gain' | 'loss' | 'dim'> = {
		fg: 'bright',
		fg2: 'dim',
		dim: 'dim',
		dim2: 'dim',
		gain: 'gain',
		loss: 'loss',
		inv: 'bright'
	};

	function toggleHud(key: string) {
		visibleHudKeys = visibleHudKeys.includes(key)
			? visibleHudKeys.filter((k) => k !== key)
			: [...visibleHudKeys, key];
	}

	let clock = $state('00:00:00');
	$effect(() => {
		const update = () => {
			const d = new Date();
			clock = [d.getHours(), d.getMinutes(), d.getSeconds()]
				.map((n) => String(n).padStart(2, '0'))
				.join(':');
		};
		update();
		const id = setInterval(update, 1000);
		return () => clearInterval(id);
	});
</script>

<!--
	`data-tauri-drag-region` turns this bar into the window's drag handle, and
	gives double-click-to-maximise for free — the two behaviours a native title
	bar would have provided. Interactive children opt out with
	`data-tauri-drag-region="false"`, or a click on them would drag the window
	instead of activating them.

	The `shell-*` classes are driven by a `data-shell` attribute the desktop
	build sets on `<html>` (see apps/senken/src/gui.rs). A browser never has it,
	so nothing below changes there.
-->
<header
	data-tauri-drag-region
	class="flex h-[46px] flex-none items-stretch border-b border-border bg-chrome shell-drag"
>
	<div
		data-tauri-drag-region
		class="flex w-[208px] flex-none items-center gap-2.5 border-r border-border px-3.5"
	>
		<div class="flex size-[22px] flex-none items-center justify-center border border-ink/35">
			<div class="size-2 rotate-45 bg-foreground"></div>
		</div>
		<div class="flex flex-col gap-px whitespace-nowrap">
			<div class="text-[13px] font-semibold tracking-[0.26em] text-foreground">SENKEN</div>
			<div class="font-mono text-[7.5px] tracking-[0.3em] text-dim">RESEARCH TERMINAL</div>
		</div>
	</div>

	<div data-tauri-drag-region class="flex flex-1 items-stretch overflow-hidden">
		{#each visibleHud as w (w.key)}
			<div
				class="flex min-w-0 flex-none items-center overflow-hidden border-r border-ink/6 px-4 whitespace-nowrap"
			>
				<StatCell label={w.label} value={w.value} tone={CELL_TONE[w.tone]} />
			</div>
		{/each}
	</div>

	<!-- Interactive: opts out, or a click here would drag the window. -->
	<div data-tauri-drag-region="false" class="flex items-center gap-3 border-l border-ink/6 px-3.5">
		<Popover.Root bind:open={navPickerOpen}>
			<Popover.Trigger
				class={cn(
					'flex size-6 flex-none cursor-pointer items-center justify-center border transition-colors',
					navPickerOpen ? 'border-foreground bg-foreground text-inv' : 'border-dim text-dim2'
				)}
			>
				<SlidersHorizontalIcon class="size-[13px]" />
			</Popover.Trigger>
			<Popover.Content align="end" sideOffset={12} class="w-[226px] border-ink/18 bg-popover p-0">
				<div class="border-b border-ink/7 px-3 py-2.5 font-mono text-[8px] tracking-[0.24em] text-dim">
					NAVBAR WIDGETS
				</div>
				{#each hud as w (w.key)}
					{@const on = visibleHudKeys.includes(w.key)}
					<div class="flex items-center justify-between border-b border-ink/[4.5%] px-3 py-2">
						<span
							class={cn(
								'font-mono text-[10px] tracking-[0.14em]',
								on ? 'text-foreground' : 'text-dim2'
							)}
						>
							{w.label}
						</span>
						<Switch size="sm" checked={on} onCheckedChange={() => toggleHud(w.key)} />
					</div>
				{/each}
			</Popover.Content>
		</Popover.Root>

		<ConnectionStatus />

		<div class="font-mono text-xs tracking-[0.08em] text-foreground">{clock}</div>

		<Avatar.Root class="size-[26px] border border-ink/16">
			<Avatar.Fallback class="font-mono text-[9.5px] text-secondary-foreground">RZ</Avatar.Fallback>
		</Avatar.Root>
	</div>
</header>
