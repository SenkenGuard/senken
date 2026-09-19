<script lang="ts">
	// "ADD WIDGET…" picker: lists the effective catalog served by
	// `GET /api/dashboard/widgets/catalog`, never a hardcoded list — a
	// widget contributed later by a plugin appears here for free the
	// moment the server's own registry reports it.
	//
	// Built on bits-ui's `Command` (the same primitive the global command
	// palette uses) instead of a plain button list, so search, keyboard
	// selection and Enter-applies-the-highlighted-row all come from the one
	// primitive this app already trusts for that, rather than a second,
	// hand-rolled implementation of the same behaviour.
	import { Command as CommandPrimitive } from 'bits-ui';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Command from '$lib/components/ui/command/index.js';
	import SearchIcon from '@lucide/svelte/icons/search';
	import type { DashboardWidgetDefinition } from './api';

	let {
		open,
		catalog,
		placedTypeIds,
		onClose,
		onPick
	}: {
		open: boolean;
		catalog: DashboardWidgetDefinition[];
		/** Every widget type already placed in the workspace this picker is
		 * open for — drawn as an "Added" badge, never as a reason to hide or
		 * disable the row: placing a second instance of the same widget type
		 * is allowed, this is informational only. */
		placedTypeIds: Set<string>;
		onClose: () => void;
		onPick: (definition: DashboardWidgetDefinition) => void;
	} = $props();

	let query = $state('');
	let inputEl = $state<HTMLInputElement | null>(null);

	function reset(): void {
		query = '';
	}

	function pick(definition: DashboardWidgetDefinition): void {
		onPick(definition);
		onClose();
		reset();
	}
</script>

<Dialog.Root
	{open}
	onOpenChange={(v) => {
		if (!v) {
			onClose();
			reset();
		}
	}}
>
	<Dialog.Content
		showCloseButton={true}
		onOpenAutoFocus={(e) => {
			// bits-ui's `Dialog` only traps focus inside its content on open —
			// it does not descend into a search input on its own (the same
			// gap `command-palette.svelte` works around the same way).
			e.preventDefault();
			inputEl?.focus();
		}}
		class="top-[30%] flex max-h-[68%] w-[420px] max-w-[92%] flex-col gap-0 overflow-hidden border-ink/18 bg-card2 p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		<Dialog.Header class="flex-none flex-row items-center justify-between gap-3 space-y-0 px-4 py-[13px]">
			<Dialog.Title class="text-[13px] font-medium tracking-[0.03em] text-foreground">
				Add widget
			</Dialog.Title>
		</Dialog.Header>

		<Command.Root class="flex min-h-0 flex-1 flex-col rounded-none bg-transparent p-0">
			<div class="flex flex-none items-center gap-[11px] border-t border-b border-ink/10 px-4 py-[11px]">
				<SearchIcon class="size-[13px] flex-none text-dim2" />
				<CommandPrimitive.Input
					bind:ref={inputEl}
					bind:value={query}
					placeholder="Search widgets…"
					class="min-w-0 flex-1 rounded-none bg-transparent font-mono text-[11px] tracking-[0.04em] text-foreground outline-none placeholder:text-dim"
				/>
			</div>

			<Command.List class="min-h-0 flex-1 overflow-y-auto">
				{#each catalog as definition (definition.widget_type_id)}
					{@const added = placedTypeIds.has(definition.widget_type_id)}
					<Command.Item
						value={`${definition.title} ${definition.description} ${definition.widget_type_id}`}
						onSelect={() => pick(definition)}
						class="flex cursor-pointer flex-col items-start gap-0.5 rounded-none border-b border-ink/[0.045] px-4 py-2.5 text-left data-selected:bg-ink/7"
					>
						<div class="flex w-full items-center gap-1.5">
							<span class="font-mono text-[10px] tracking-[0.12em] text-foreground uppercase">
								{definition.title}
							</span>
							{#if added}
								<span
									class="flex-none rounded-sm border border-ink/20 px-1 py-px font-mono text-[8px] font-semibold tracking-[0.12em] text-dim uppercase"
									data-added-label
								>
									Added
								</span>
							{/if}
							{#if definition.data_source === 'mock'}
								<span
									class="flex-none rounded-sm border border-ink/20 px-1 py-px font-mono text-[8px] font-semibold tracking-[0.12em] text-dim uppercase"
									data-mockup-label
									title="This widget renders seeded example data, not a real account."
								>
									Mockup
								</span>
							{/if}
						</div>
						<span class="font-mono text-[8.5px] tracking-[0.1em] text-dim">
							{definition.description}
						</span>
					</Command.Item>
				{/each}
				<Command.Empty
					class="flex flex-col items-center gap-2 px-4 py-8 text-center font-mono text-[9px] tracking-[0.12em] text-dim uppercase"
				>
					No widgets match
					<button
						type="button"
						class="cursor-pointer border border-ink/20 px-2 py-1 font-mono text-[8.5px] tracking-[0.12em] text-secondary-foreground normal-case hover:bg-ink/7"
						onclick={reset}
					>
						Clear search
					</button>
				</Command.Empty>
			</Command.List>
		</Command.Root>
	</Dialog.Content>
</Dialog.Root>
