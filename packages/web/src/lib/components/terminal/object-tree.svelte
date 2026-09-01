<script lang="ts">
	// The OBJECT TREE tab's right panel: a pane's instruments, indicators and
	// drawings, grouped and collapsible (`ui/accordion`, groups start
	// expanded), each row a real set of controls — show/hide, options, move
	// up/down, remove — rather than the static eye icon this replaced.
	// Follows `draw-toolbar.svelte`'s Tooltip.Provider + Tooltip.Root/Trigger/
	// Content pattern for every icon button's label.
	import { untrack } from 'svelte';
	import { neighborInGroup, type ObjectTreeGroup, type ObjectTreeItem } from '$lib/charts/pane-runtime';
	import { cn } from '$lib/utils.js';
	import * as Accordion from '$lib/components/ui/accordion/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import EyeIcon from '@lucide/svelte/icons/eye';
	import EyeOffIcon from '@lucide/svelte/icons/eye-off';
	import SlidersIcon from '@lucide/svelte/icons/sliders-horizontal';
	import ChevronUpIcon from '@lucide/svelte/icons/chevron-up';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import TrashIcon from '@lucide/svelte/icons/trash';

	let {
		groups,
		selectedId,
		onSelect,
		onToggle,
		onOptions,
		onRemove,
		onMove
	}: {
		groups: ObjectTreeGroup[];
		selectedId: string | null;
		onSelect: (item: ObjectTreeItem) => void;
		onToggle: (item: ObjectTreeItem) => void;
		onOptions: (item: ObjectTreeItem) => void;
		onRemove: (item: ObjectTreeItem) => void;
		/** Swaps `item` with `neighborId` — always another row from `item`'s own
		 * group (`neighborInGroup` below computes it from that group's ordered
		 * ids alone), never a row from a different group. Passing the id
		 * directly, rather than a bare direction, is what keeps a reorder from
		 * ever crossing a group boundary: the caller has no index of its own to
		 * get wrong. */
		onMove: (item: ObjectTreeItem, neighborId: string) => void;
	} = $props();

	const flyoutClass = 'rounded-none border border-ink/16 bg-popover px-2.5 py-1.5 text-left shadow-[0_12px_28px_rgba(0,0,0,0.6)]';

	// Groups start expanded — seeded once from the group keys, which are
	// fixed (INSTRUMENTS/INDICATORS/DRAWINGS) for the panel's whole
	// lifetime, so this deliberately does not track later `groups` updates:
	// a group the user has collapsed must stay collapsed as its own items
	// come and go.
	let expanded = $state(untrack(() => groups.map((g) => g.key)));
</script>

<Tooltip.Provider>
	<Accordion.Root type="multiple" bind:value={expanded} class="flex-1">
		{#each groups as g (g.key)}
			<Accordion.Item value={g.key} class="border-b border-ink/6">
				<Accordion.Trigger
					level={3}
					class="flex w-full cursor-pointer items-center justify-between bg-ink/[0.03] px-3 py-1.5 hover:no-underline"
				>
					<span class="font-mono text-[8px] tracking-[0.24em] text-dim">{g.title}</span>
					<span class="font-mono text-[9px] text-dim">{g.items.length}</span>
				</Accordion.Trigger>
				<Accordion.Content class="p-0">
					{#each g.items as item, i (item.id)}
						{@const groupIds = g.items.map((it) => it.id)}
						{@const selected = item.kind === 'drawing' && item.id === selectedId}
						<div
							class={cn(
								'flex items-center justify-between gap-2 border-l-2 py-1.5 pr-2 pl-4',
								selected ? 'border-foreground bg-ink/6' : 'border-transparent'
							)}
						>
							<button
								type="button"
								class="flex min-w-0 flex-1 cursor-pointer items-baseline gap-2 text-left"
								onclick={() => onSelect(item)}
								aria-label={`Select ${item.name}`}
							>
								<span
									class={cn(
										'font-mono text-[10px] tracking-[0.08em] whitespace-nowrap',
										item.visible ? 'text-secondary-foreground' : 'text-dim'
									)}
								>
									{item.name}
								</span>
								<span class="truncate font-mono text-[8.5px] text-dim">{item.params}</span>
							</button>

							<div class="flex flex-none items-center gap-0.5">
								<Tooltip.Root>
									<Tooltip.Trigger
										class="flex size-5 cursor-pointer items-center justify-center text-dim2 hover:text-foreground"
										onclick={() => onToggle(item)}
										aria-label={item.visible ? `Hide ${item.name}` : `Show ${item.name}`}
									>
										{#if item.visible}
											<EyeIcon class="size-3" />
										{:else}
											<EyeOffIcon class="size-3" />
										{/if}
									</Tooltip.Trigger>
									<Tooltip.Content side="top" sideOffset={6} arrowClasses="hidden" class={flyoutClass}>
										<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">
											{item.visible ? 'HIDE' : 'SHOW'}
										</span>
									</Tooltip.Content>
								</Tooltip.Root>

								{#if item.canConfigure}
									<Tooltip.Root>
										<Tooltip.Trigger
											class="flex size-5 cursor-pointer items-center justify-center text-dim2 hover:text-foreground"
											onclick={() => onOptions(item)}
											aria-label={`Options for ${item.name}`}
										>
											<SlidersIcon class="size-3" />
										</Tooltip.Trigger>
										<Tooltip.Content side="top" sideOffset={6} arrowClasses="hidden" class={flyoutClass}>
											<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">OPTIONS</span>
										</Tooltip.Content>
									</Tooltip.Root>
								{/if}

								<Tooltip.Root>
									<Tooltip.Trigger
										class="flex size-5 cursor-pointer items-center justify-center text-dim2 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40"
										onclick={() => {
											const neighbor = neighborInGroup(groupIds, item.id, 'up');
											if (neighbor) onMove(item, neighbor);
										}}
										disabled={i === 0}
										aria-label={`Move ${item.name} up`}
									>
										<ChevronUpIcon class="size-3" />
									</Tooltip.Trigger>
									<Tooltip.Content side="top" sideOffset={6} arrowClasses="hidden" class={flyoutClass}>
										<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">MOVE UP</span>
									</Tooltip.Content>
								</Tooltip.Root>

								<Tooltip.Root>
									<Tooltip.Trigger
										class="flex size-5 cursor-pointer items-center justify-center text-dim2 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40"
										onclick={() => {
											const neighbor = neighborInGroup(groupIds, item.id, 'down');
											if (neighbor) onMove(item, neighbor);
										}}
										disabled={i === g.items.length - 1}
										aria-label={`Move ${item.name} down`}
									>
										<ChevronDownIcon class="size-3" />
									</Tooltip.Trigger>
									<Tooltip.Content side="top" sideOffset={6} arrowClasses="hidden" class={flyoutClass}>
										<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">MOVE DOWN</span>
									</Tooltip.Content>
								</Tooltip.Root>

								<Tooltip.Root>
									<Tooltip.Trigger
										class="flex size-5 cursor-pointer items-center justify-center text-dim2 hover:text-destructive"
										onclick={() => onRemove(item)}
										aria-label={`Remove ${item.name}`}
									>
										<TrashIcon class="size-3" />
									</Tooltip.Trigger>
									<Tooltip.Content side="top" sideOffset={6} arrowClasses="hidden" class={flyoutClass}>
										<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">REMOVE</span>
									</Tooltip.Content>
								</Tooltip.Root>
							</div>
						</div>
					{/each}
				</Accordion.Content>
			</Accordion.Item>
		{/each}
	</Accordion.Root>
</Tooltip.Provider>
