<script lang="ts">
	// Global command palette — mounted
	// once in `AppShell` per Part C ("global chrome... in layout/"), driven
	// by `$lib/state/command-palette.svelte`. Every page opens it in a
	// different `mode` (line 2716): the dashboard's "ADD WIDGET…"
	// (`'widget'`), the trade engine's "ATTACH ACCOUNT" / account chip
	// (`'adapter'` / `'account'`), and the charts page's symbol readout /
	// "INDICATORS & LAYERS" (`'symbol'` / `'layer'`).
	//
	// This is also the reference's *symbol picker* — see
	// `ui/color-picker/color-picker.svelte`'s header comment for why lines
	// 1108-1140 (what the line map calls "Symbol picker") are not it.
	//
	// Reuses shadcn `command` inside `dialog`.
	// `Command.Root` gets `shouldFilter={false}`: row filtering is done by
	// each caller's own `rows(query)` closure (reference: the `cmdRows`
	// derivation per `s.cmd`, lines 2716-2753), not cmdk's built-in fuzzy
	// match, so the two would otherwise double-filter.
	//
	// the reference's own `cmdRows` markup (lines 1141-1177) is
	// static — plain `onClick` divs, no hover state, no keyboard handling —
	// because a design canvas has no interaction model. bits-ui's `Command`
	// primitive already drives a real one underneath it for free (arrow keys
	// move `data-selected` between rows, hovering does the same, Enter
	// clicks the selected row — all confirmed live via
	// `hasAttribute('data-selected')` before touching anything), so the
	// "list you can only click" bug was purely cosmetic, not missing
	// wiring: `border-ink/18` on `Dialog.Content` sets a border *color* with
	// no `border` (width) utility alongside it, so there was no line to see
	// (`getComputedStyle` showed `0px solid …`) — fixed by adding `border`.
	// And `data-selected:bg-muted` on `ui/command`'s own `Command.Item`
	// resolves to `--card2`, the exact color this dialog's own
	// `bg-card2` already paints behind it, so the "selected" row was always
	// rendering, just in a color identical to its background — fixed here
	// with `data-selected:bg-ink/7`, the same subtle-highlight token this
	// app already uses for hover/focus rows elsewhere (nav-rail, the
	// workspace menu, pane-header's layer chips).
	import { Command as CommandPrimitive } from 'bits-ui';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Command from '$lib/components/ui/command/index.js';
	import { cn } from '$lib/utils.js';
	import { commandPalette, closeCommand } from '$lib/state/command-palette.svelte';
	import SearchIcon from '@lucide/svelte/icons/search';

	const toneClass = {
		fg: 'text-foreground',
		fg2: 'text-secondary-foreground',
		dim: 'text-dim',
		dim2: 'text-dim2',
		gain: 'text-gain',
		loss: 'text-loss',
		inv: 'text-inv'
	} as const;

	const rows = $derived(commandPalette.request?.rows(commandPalette.query) ?? []);

	// bits-ui's Dialog only traps focus inside its content on open — it
	// does not descend into a search input the way a command palette needs
	// to (verified: without this, the dialog opens with keyboard input
	// going nowhere until the user clicks the field). `onOpenAutoFocus`
	// lets us redirect that initial focus onto the input ourselves.
	let inputEl = $state<HTMLInputElement | null>(null);
</script>

<Dialog.Root
	bind:open={commandPalette.open}
	onOpenChange={(v) => {
		if (!v) closeCommand();
	}}
>
	<Dialog.Content
		showCloseButton={false}
		onOpenAutoFocus={(e) => {
			e.preventDefault();
			inputEl?.focus();
		}}
		class="top-[84px] flex max-h-[68%] w-[592px] max-w-[92%] translate-y-0 flex-col gap-0 overflow-hidden border border-ink/18 bg-card2 p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		<Dialog.Header class="sr-only">
			<Dialog.Title>Command palette</Dialog.Title>
			<Dialog.Description>Search {commandPalette.request?.mode ?? ''} commands</Dialog.Description>
		</Dialog.Header>

		<Command.Root shouldFilter={false} class="flex h-full min-h-0 flex-col rounded-none bg-transparent p-0">
			<div class="flex flex-none items-center gap-[11px] border-b border-ink/10 px-[15px] py-[13px]">
				<SearchIcon class="size-[15px] flex-none text-dim2" />
				<CommandPrimitive.Input
					bind:ref={inputEl}
					bind:value={commandPalette.query}
					placeholder={commandPalette.request?.placeholder ?? 'Search…'}
					class="min-w-0 flex-1 bg-transparent font-mono text-[13px] tracking-[0.04em] text-foreground outline-none placeholder:text-dim"
				/>
				<span class="flex-none border border-ink/14 px-1.5 py-[3px] font-mono text-[8px] tracking-[0.2em] text-dim">
					ESC
				</span>
			</div>

			{#if commandPalette.request?.kindTabs}
				<div class="flex flex-none border-b border-ink/8">
					{#each commandPalette.request.kindTabs as k (k.label)}
						<button
							type="button"
							class={cn(
								'flex-1 cursor-pointer py-[9px] text-center font-mono text-[8.5px] tracking-[0.18em]',
								k.active() ? 'bg-foreground text-inv' : 'text-dim2'
							)}
							onclick={k.onClick}
						>
							{k.label}
						</button>
					{/each}
				</div>
			{/if}

			<Command.List class="max-h-none min-h-0 flex-1 overflow-auto">
				{#each rows as r (r.title + r.sub)}
					<Command.Item
						value={r.title + ' ' + r.sub}
						onSelect={r.onPick}
						class="flex cursor-pointer items-center gap-3 rounded-none border-b border-ink/5 px-[15px] py-[11px] data-selected:bg-ink/7"
					>
						<r.icon class="size-[15px] flex-none text-dim2" />
						<div class="flex min-w-0 flex-1 flex-col gap-0.5">
							<span class="truncate font-mono text-xs tracking-[0.04em] text-foreground">{r.title}</span>
							<span class="truncate font-mono text-[9px] text-dim">{r.sub}</span>
						</div>
						<span class={cn('flex-none font-mono text-[10.5px]', toneClass[r.metaTone])}>{r.meta}</span>
					</Command.Item>
				{/each}
				{#if rows.length === 0}
					<Command.Empty class="px-[15px] py-[26px] text-center font-mono text-[10px] tracking-[0.16em] text-dim">
						NO MATCH
					</Command.Empty>
				{/if}
			</Command.List>

			<div
				class="flex flex-none items-center justify-between border-t border-ink/8 px-[15px] py-[9px] font-mono text-[8px] tracking-[0.18em] text-dim"
			>
				<span>{commandPalette.request?.footer ?? ''}</span>
				<span>ENTER TO APPLY</span>
			</div>
		</Command.Root>
	</Dialog.Content>
</Dialog.Root>
