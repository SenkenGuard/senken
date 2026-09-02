<script lang="ts">
	// 46px shared chrome header: wordmark,
	// the HUD stat strip, the nav-widget picker, a live clock and the account
	// avatar. Visually distinct from NavRail, which is the 52px sidebar below
	// it (lines 76-101) — the reference nests both under one flex column, but
	// they are two different pieces of chrome and two different components
	// here.
	import { cn } from '$lib/utils.js';
	import { HUD_WIDGETS, DEFAULT_HUD_KEYS } from '$lib/mock/shell';
	import { StatCell } from '$lib/components/ui/stat-cell/index.js';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import * as Avatar from '$lib/components/ui/avatar/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import ConnectionStatus from './connection-status.svelte';
	import SlidersHorizontalIcon from '@lucide/svelte/icons/sliders-horizontal';
	import { formatInstant } from '$lib/time';
	import { userZoneStore } from '$lib/state/user-zone.svelte';

	let navPickerOpen = $state(false);
	let visibleHudKeys = $state<string[]>([...DEFAULT_HUD_KEYS]);
	const visibleHud = $derived(HUD_WIDGETS.filter((w) => visibleHudKeys.includes(w.key)));

	function toggleHud(key: string) {
		visibleHudKeys = visibleHudKeys.includes(key)
			? visibleHudKeys.filter((k) => k !== key)
			: [...visibleHudKeys, key];
	}

	// "Now", in the viewer's chosen display zone rather than the machine's
	// own — the same door every other on-screen instant in this app renders
	// through (`$lib/time`'s `formatInstant`), so this clock and a chart's
	// time axis never disagree about what zone "now" is in.
	let clock = $state('00:00:00');
	$effect(() => {
		const zoneId = userZoneStore.zone;
		const update = () => {
			clock = formatInstant(Date.now() * 1_000_000, zoneId).text.slice(11);
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
				<StatCell label={w.label} value={w.value} tone={w.tone} />
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
				{#each HUD_WIDGETS as w (w.key)}
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
