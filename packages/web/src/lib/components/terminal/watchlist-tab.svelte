<script lang="ts">
	// The right panel's WATCHLIST tab, replacing the old inline instrument
	// picker (`+page.svelte`'s former `watch` branch — a search box with live
	// prices, no saved membership at all). Watchlist groups and their
	// members are global per user (`senken_watchlist::WatchlistStore`,
	// `$lib/api/client.ts`'s own doc); which of them show *in this
	// workspace* is local, stored in the workspace's own `settings` text
	// (`$lib/charts/workspace-settings.ts`, `chartWorkspaceStore.workspaceSettings`).
	//
	// This component owns the group list, the GROUPS popover that creates/
	// renames/deletes/reorders groups and toggles which are shown, and the
	// one lease that covers every shown group's members at once.
	// `watchlist-group-section.svelte` owns one shown group's own membership
	// and its add-instrument search, reporting its instrument set back up
	// through `onMembersChange` so this can compute that lease.
	import { onMount, untrack } from 'svelte';
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import { wsClient } from '$lib/api/websocket';
	import { wsEventsStore, priceFromEvent } from '$lib/api/ws-events.svelte';
	import { chartWorkspaceStore, updateWorkspaceSettings } from '$lib/charts/workspace-store.svelte';
	import { resolveShown } from '$lib/charts/workspace-settings';
	import { swapById } from '$lib/charts/pane-runtime';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import * as Accordion from '$lib/components/ui/accordion/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import WatchlistGroupSection from './watchlist-group-section.svelte';
	import type { WatchlistGroupDto } from '$lib/api/types';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import ChevronUpIcon from '@lucide/svelte/icons/chevron-up';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';

	let {
		onPickInstrument
	}: {
		/** Sets the active pane's instrument — `+page.svelte`'s
		 * `pickPaneInstrument`, the pane-targeting core `pickSymbol` itself
		 * calls. A watchlist row only ever has the bare instrument id (a
		 * `WatchlistMemberDto` carries no market-status field the way an
		 * `InstrumentSummaryDto` search hit does), so this takes just the id
		 * rather than a whole summary — the page's own pane-instrument
		 * resolution effect fills in market status for it moments later,
		 * the same way it already does for every pane loaded from a saved
		 * workspace. */
		onPickInstrument: (instrument: string) => void;
	} = $props();

	const flyoutClass = 'rounded-none border border-ink/16 bg-popover px-2.5 py-1.5 text-left shadow-[0_12px_28px_rgba(0,0,0,0.6)]';

	// --- Owned groups + the GROUPS popover -------------------------------

	let groups = $state<WatchlistGroupDto[]>([]);
	let groupsLoading = $state(true);
	let groupsError = $state<string | null>(null);

	async function loadGroups(): Promise<void> {
		groupsLoading = true;
		groupsError = null;
		try {
			const page = await apiClient.listWatchlistGroups(200, 0);
			groups = [...page.rows].sort((a, b) => a.position - b.position);
		} catch (error) {
			groupsError = getErrorMessage(error, 'Could not load your watchlist groups.');
		} finally {
			groupsLoading = false;
		}
	}

	onMount(() => void loadGroups());

	const shownIds = $derived(chartWorkspaceStore.workspaceSettings.watchlistGroups);
	const shownGroups = $derived(resolveShown(groups, shownIds));

	let groupsMenuOpen = $state(false);
	let newGroupName = $state('');
	let creatingGroup = $state(false);
	let createGroupError = $state<string | null>(null);

	async function submitNewGroup(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		const name = newGroupName.trim();
		if (!name) return;
		creatingGroup = true;
		createGroupError = null;
		try {
			await apiClient.createWatchlistGroup({ name });
			newGroupName = '';
			await loadGroups();
		} catch (error) {
			createGroupError = getErrorMessage(error, 'Could not create that group.');
		} finally {
			creatingGroup = false;
		}
	}

	let renamingGroupId = $state<string | null>(null);
	let renameDraft = $state('');
	let renameGroupError = $state<string | null>(null);

	function startRenameGroup(id: string, name: string): void {
		renamingGroupId = id;
		renameDraft = name;
	}

	async function submitRenameGroup(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		const id = renamingGroupId;
		const name = renameDraft.trim();
		renamingGroupId = null;
		if (!id || !name) return;
		renameGroupError = null;
		try {
			await apiClient.renameWatchlistGroup(id, name);
			await loadGroups();
		} catch (error) {
			renameGroupError = getErrorMessage(error, 'Could not rename that group.');
		}
	}

	let deleteGroupError = $state<string | null>(null);

	/** Deletes a group and, since it can no longer be shown anywhere once it
	 * no longer exists, drops its id from this workspace's own shown list
	 * too — otherwise the id would sit in `workspaceSettings.watchlistGroups`
	 * forever, exactly the "shown id nobody owns any more" case
	 * `resolveShown` has to tolerate for every *other* workspace that had it
	 * shown. */
	async function deleteGroup(id: string): Promise<void> {
		deleteGroupError = null;
		try {
			await apiClient.deleteWatchlistGroup(id);
			await loadGroups();
			if (shownIds.includes(id)) {
				await updateWorkspaceSettings({ watchlistGroups: shownIds.filter((gid) => gid !== id) });
			}
		} catch (error) {
			deleteGroupError = getErrorMessage(error, 'Could not delete that group.');
		}
	}

	let reorderGroupsError = $state<string | null>(null);

	async function moveGroup(id: string, direction: 'up' | 'down'): Promise<void> {
		const index = groups.findIndex((g) => g.id === id);
		const neighbor = groups[direction === 'up' ? index - 1 : index + 1];
		if (index === -1 || !neighbor) return;
		const reordered = swapById(groups, id, neighbor.id);
		groups = reordered;
		reorderGroupsError = null;
		try {
			await apiClient.reorderWatchlistGroups(reordered.map((g) => g.id));
		} catch (error) {
			reorderGroupsError = getErrorMessage(error, 'Could not reorder your groups.');
			await loadGroups();
		}
	}

	function toggleShownGroup(id: string): void {
		const next = shownIds.includes(id) ? shownIds.filter((gid) => gid !== id) : [...shownIds, id];
		void updateWorkspaceSettings({ watchlistGroups: next });
	}

	// --- Shown-section expansion ------------------------------------------
	//
	// Groups start expanded, same as `object-tree.svelte`'s own panel — but
	// unlike that panel's fixed three groups, which shown group exists here
	// changes as groups load, get toggled shown, or get deleted. Each id is
	// seeded into `expandedGroups` (open) exactly once, the first time it
	// appears in `shownGroups`; a later collapse the user makes is never
	// forced back open by a later, unrelated change to the shown set.
	let expandedGroups = $state<string[]>([]);
	const seededGroupIds = new Set<string>();
	$effect(() => {
		const ids = shownGroups.map((g) => g.id);
		untrack(() => {
			const newlyShown = ids.filter((id) => !seededGroupIds.has(id));
			if (newlyShown.length === 0) return;
			for (const id of newlyShown) seededGroupIds.add(id);
			expandedGroups = [...expandedGroups, ...newlyShown];
		});
	});

	// --- The one lease covering every shown group's members --------------
	//
	// Each `WatchlistGroupSection` reports its own current instrument set
	// here through `onMembersChange`; `leasedInstruments` folds every
	// *currently shown* group's set into one list (a group hidden, or
	// deleted, stops contributing the moment it drops out of `shownGroups`,
	// even if its stale entry is still sitting in `instrumentsByGroup`).
	// The lease itself follows the same take-in-the-effect-body,
	// release-in-its-cleanup discipline the former `watch` branch used: see
	// its own comment, preserved here.
	let instrumentsByGroup = $state<Record<string, string[]>>({});
	function reportGroupInstruments(groupId: string, instruments: string[]): void {
		instrumentsByGroup = { ...instrumentsByGroup, [groupId]: instruments };
	}
	const leasedInstruments = $derived(
		Array.from(new Set(shownGroups.flatMap((g) => instrumentsByGroup[g.id] ?? [])))
	);

	// The lease itself: subscribing here and unsubscribing in this effect's
	// own cleanup (fired again whenever `leasedInstruments` changes, and
	// once more on destroy — i.e. whenever this whole tab unmounts, since
	// the charts route only mounts this component while the WATCHLIST tab
	// is the open one) is the whole release mechanism — nobody has to
	// remember to call anything, the same discipline `chart-pane.svelte`'s
	// own live-price effect follows for a pane.
	$effect(() => {
		const ids = leasedInstruments;
		for (const id of ids) wsClient.subscribe(id);
		return () => {
			for (const id of ids) wsClient.unsubscribe(id);
		};
	});

	let prices = $state<Record<string, number>>({});
	$effect(() => {
		const event = wsEventsStore.last;
		if (!event) return;
		const ids = leasedInstruments;
		// `prices` is read and written inside `untrack` deliberately — this
		// effect's only reactive dependencies are the incoming WS event and
		// the currently-leased instruments, exactly the reasoning the former
		// `watch` branch's own live-price effect documented for the same
		// shape of update.
		untrack(() => {
			let next: Record<string, number> | null = null;
			for (const id of ids) {
				const price = priceFromEvent(event, id);
				if (price != null && prices[id] !== price) {
					next ??= { ...prices };
					next[id] = price;
				}
			}
			if (next) prices = next;
		});
	});
</script>

<div class="flex min-h-0 flex-1 flex-col">
	<div class="flex flex-none items-center justify-between gap-2 border-b border-ink/7 px-3 py-2">
		<span class="font-mono text-[8px] tracking-[0.26em] text-dim">SHOWN GROUPS</span>
		<Tooltip.Provider>
			<Popover.Root bind:open={groupsMenuOpen}>
				<Tooltip.Root>
					<Tooltip.Trigger>
						{#snippet child({ props })}
							<span {...props} class="contents">
								<Popover.Trigger
									class="flex h-6 cursor-pointer items-center gap-1.5 border border-ink/14 px-2"
								>
									<span class="font-mono text-[9px] tracking-[0.14em] text-dim2">GROUPS</span>
									<ChevronDownIcon class="size-3 text-dim2" />
								</Popover.Trigger>
							</span>
						{/snippet}
					</Tooltip.Trigger>
					<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class={flyoutClass}>
						<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">MANAGE WATCHLIST GROUPS</span>
					</Tooltip.Content>
				</Tooltip.Root>
				<Popover.Content align="end" sideOffset={10} class="w-[280px] rounded-none border-ink/16 bg-popover p-0">
					<form onsubmit={submitNewGroup} class="flex items-center gap-1.5 border-b border-ink/7 px-3 py-2.5">
						<Input placeholder="New group name…" bind:value={newGroupName} class="h-7 flex-1 text-[11px]" />
						<Tooltip.Root>
							<Tooltip.Trigger
								type="submit"
								class="flex size-7 flex-none cursor-pointer items-center justify-center border border-ink/14 text-dim2 disabled:cursor-not-allowed disabled:opacity-40"
								disabled={creatingGroup}
								aria-label="Create group"
							>
								<PlusIcon class="size-3" />
							</Tooltip.Trigger>
							<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class={flyoutClass}>
								<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">CREATE GROUP</span>
							</Tooltip.Content>
						</Tooltip.Root>
					</form>
					{#if createGroupError}
						<p class="px-3 pb-2 text-[10.5px] text-destructive">{createGroupError}</p>
					{/if}
					<div class="px-3 pt-2.5 pb-1.5 font-mono text-[8px] tracking-[0.26em] text-dim">YOUR GROUPS</div>
					{#if groupsLoading}
						<p class="px-3 pb-2.5 font-mono text-[9px] tracking-[0.16em] text-dim">LOADING…</p>
					{:else if groups.length === 0}
						<p class="px-3 pb-2.5 font-mono text-[9px] tracking-[0.16em] text-dim">NO GROUPS YET</p>
					{/if}
					{#each groups as g, i (g.id)}
						<div class="mx-1.5 mb-1 flex items-center gap-1.5 border border-transparent px-2 py-1.5">
							<Switch
								size="sm"
								checked={shownIds.includes(g.id)}
								onCheckedChange={() => toggleShownGroup(g.id)}
								aria-label={`Show ${g.name} in this workspace`}
							/>
							{#if renamingGroupId === g.id}
								<form onsubmit={submitRenameGroup} class="flex flex-1 items-center gap-1">
									<Input bind:value={renameDraft} class="h-6 flex-1 text-[11px]" autofocus />
									<button type="submit" class="font-mono text-[9px] text-dim2">OK</button>
								</form>
							{:else}
								<span class="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground">{g.name}</span>
								<Tooltip.Root>
									<Tooltip.Trigger
										class="flex-none cursor-pointer text-dim2"
										onclick={() => startRenameGroup(g.id, g.name)}
										aria-label={`Rename ${g.name}`}
									>
										<PencilIcon class="size-3" />
									</Tooltip.Trigger>
									<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class={flyoutClass}>
										<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">RENAME GROUP</span>
									</Tooltip.Content>
								</Tooltip.Root>
								<Tooltip.Root>
									<Tooltip.Trigger
										class="flex-none cursor-pointer text-dim2 disabled:cursor-not-allowed disabled:opacity-40"
										disabled={i === 0}
										onclick={() => moveGroup(g.id, 'up')}
										aria-label={`Move ${g.name} up`}
									>
										<ChevronUpIcon class="size-3" />
									</Tooltip.Trigger>
									<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class={flyoutClass}>
										<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">MOVE UP</span>
									</Tooltip.Content>
								</Tooltip.Root>
								<Tooltip.Root>
									<Tooltip.Trigger
										class="flex-none cursor-pointer text-dim2 disabled:cursor-not-allowed disabled:opacity-40"
										disabled={i === groups.length - 1}
										onclick={() => moveGroup(g.id, 'down')}
										aria-label={`Move ${g.name} down`}
									>
										<ChevronDownIcon class="size-3" />
									</Tooltip.Trigger>
									<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class={flyoutClass}>
										<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">MOVE DOWN</span>
									</Tooltip.Content>
								</Tooltip.Root>
								<Tooltip.Root>
									<Tooltip.Trigger
										class="flex-none cursor-pointer text-dim2 hover:text-destructive"
										onclick={() => deleteGroup(g.id)}
										aria-label={`Delete ${g.name}`}
									>
										<Trash2Icon class="size-3" />
									</Tooltip.Trigger>
									<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class={flyoutClass}>
										<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">DELETE GROUP</span>
									</Tooltip.Content>
								</Tooltip.Root>
							{/if}
						</div>
					{/each}
					{#if renameGroupError}
						<p class="px-3 pb-2 text-[10.5px] text-destructive">{renameGroupError}</p>
					{/if}
					{#if deleteGroupError}
						<p class="px-3 pb-2 text-[10.5px] text-destructive">{deleteGroupError}</p>
					{/if}
					{#if reorderGroupsError}
						<p class="px-3 pb-2 text-[10.5px] text-destructive">{reorderGroupsError}</p>
					{/if}
				</Popover.Content>
			</Popover.Root>
		</Tooltip.Provider>
	</div>

	<div class="min-h-0 flex-1 overflow-auto">
		{#if groupsLoading}
			<p class="py-5.5 text-center font-mono text-[9px] tracking-[0.16em] text-dim">LOADING WATCHLIST…</p>
		{:else if groupsError}
			<div class="m-3 border border-destructive/30 bg-destructive/10 px-2.5 py-2 text-destructive">
				<p class="text-[11px]">{groupsError}</p>
			</div>
		{:else if shownGroups.length === 0}
			<div class="px-4 py-6 text-center">
				<p class="font-mono text-[9.5px] leading-relaxed text-dim">
					No watchlist shown in this workspace — pick one from Groups, or create one.
				</p>
			</div>
		{:else}
			<Accordion.Root type="multiple" bind:value={expandedGroups} class="flex-1">
				{#each shownGroups as g (g.id)}
					<WatchlistGroupSection
						group={g}
						{prices}
						{onPickInstrument}
						onMembersChange={reportGroupInstruments}
					/>
				{/each}
			</Accordion.Root>
		{/if}
	</div>
</div>
