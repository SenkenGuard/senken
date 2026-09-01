<script lang="ts">
	// One shown group inside `watchlist-tab.svelte`'s accordion: its own
	// membership (load, remove), and the small search box that adds an
	// instrument to it. Split out of the tab itself the same reason
	// `chart-context-menu.svelte`-style child components exist elsewhere in
	// this route — the tab owns which groups are *shown*, this owns what one
	// shown group *contains*, and the two only meet through
	// `onMembersChange` (this group's current instrument set, reported up so
	// the tab can compute the one lease that covers every shown group at
	// once — see its own header comment).
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import * as Accordion from '$lib/components/ui/accordion/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { parseInstrumentId, fmtDecimal } from './chart-config';
	import type { WatchlistGroupDto, WatchlistMemberDto, InstrumentSummaryDto } from '$lib/api/types';
	import XIcon from '@lucide/svelte/icons/x';
	import CoinsIcon from '@lucide/svelte/icons/coins';

	let {
		group,
		prices,
		onPickInstrument,
		onMembersChange
	}: {
		group: WatchlistGroupDto;
		/** Live prices keyed by instrument id, shared across every shown
		 * group — the tab holds the one lease/price map for all of them, this
		 * only reads whichever entries belong to its own members. */
		prices: Record<string, number>;
		onPickInstrument: (instrument: string) => void;
		/** Reports this group's current member instruments after every load
		 * or mutation, so the tab's lease always matches what is actually on
		 * screen — not just what it was when the tab opened. */
		onMembersChange: (groupId: string, instruments: string[]) => void;
	} = $props();

	let members = $state<WatchlistMemberDto[]>([]);
	let loading = $state(true);
	let loadError = $state<string | null>(null);

	async function loadMembers(): Promise<void> {
		loading = true;
		loadError = null;
		try {
			const rows = await apiClient.listWatchlistMembers(group.id);
			members = [...rows].sort((a, b) => a.position - b.position);
			onMembersChange(group.id, members.map((m) => m.instrument));
		} catch (error) {
			loadError = getErrorMessage(error, 'Could not load this group.');
		} finally {
			loading = false;
		}
	}

	onMount(() => void loadMembers());

	let removingId = $state<string | null>(null);
	let removeError = $state<string | null>(null);

	async function removeMember(member: WatchlistMemberDto): Promise<void> {
		removingId = member.id;
		removeError = null;
		try {
			await apiClient.removeWatchlistMember(member.id);
			await loadMembers();
		} catch (error) {
			removeError = getErrorMessage(error, 'Could not remove that instrument.');
		} finally {
			removingId = null;
		}
	}

	// Add-to-group search. Unlike the toolbar's instrument pickers, this
	// stays empty until the user types rather than opening with an
	// unfiltered catalogue page — one of these boxes exists per shown group,
	// and firing a search for every one of them before anybody has touched
	// the keyboard would be a request per group for nothing shown yet.
	let query = $state('');
	let results = $state<InstrumentSummaryDto[]>([]);
	let searchToken = 0;
	$effect(() => {
		const q = query.trim();
		const token = ++searchToken;
		if (!q) {
			results = [];
			return;
		}
		apiClient
			.searchInstruments(q, 8)
			.then((page) => {
				if (token === searchToken) results = page.rows;
			})
			.catch(() => {
				if (token === searchToken) results = [];
			});
	});

	let addingId = $state<string | null>(null);
	let addError = $state<string | null>(null);

	async function addMember(instrument: InstrumentSummaryDto): Promise<void> {
		addingId = instrument.id;
		addError = null;
		try {
			// `addWatchlistMember`'s own doc: adding an instrument the group
			// already holds resolves with that member's id rather than
			// conflicting, so there is no membership check to make here first.
			await apiClient.addWatchlistMember(group.id, { instrument: instrument.id });
			query = '';
			results = [];
			await loadMembers();
		} catch (error) {
			addError = getErrorMessage(error, 'Could not add that instrument.');
		} finally {
			addingId = null;
		}
	}
</script>

<Accordion.Item value={group.id} class="border-b border-ink/6">
	<Accordion.Trigger
		level={3}
		class="flex w-full cursor-pointer items-center justify-between bg-ink/[0.03] px-3 py-1.5 hover:no-underline"
	>
		<span class="truncate font-mono text-[8px] tracking-[0.24em] text-dim">{group.name.toUpperCase()}</span>
		<span class="flex-none font-mono text-[9px] text-dim">{members.length}</span>
	</Accordion.Trigger>
	<Accordion.Content class="p-0">
		{#if loading}
			<p class="px-3 py-2 font-mono text-[9px] tracking-[0.16em] text-dim">LOADING…</p>
		{:else if loadError}
			<p class="px-3 py-2 text-[10.5px] text-destructive">{loadError}</p>
		{:else}
			{#each members as m (m.id)}
				{@const parsed = parseInstrumentId(m.instrument)}
				<div class="flex items-center justify-between gap-2 border-b border-ink/5 px-3 py-2">
					<button
						type="button"
						class="flex min-w-0 flex-1 cursor-pointer items-center gap-2.5 text-left"
						onclick={() => onPickInstrument(m.instrument)}
					>
						<div class="flex size-[18px] flex-none items-center justify-center border border-ink/14 font-mono text-[8.5px] text-secondary-foreground">
							{parsed.ticker.slice(0, 1)}
						</div>
						<span class="truncate font-mono text-[10.5px] tracking-[0.06em] text-foreground">{parsed.venue}:{parsed.ticker}</span>
					</button>
					<span class="flex-none font-mono text-[10px] text-secondary-foreground">
						{prices[m.instrument] != null ? fmtDecimal(prices[m.instrument]) : ''}
					</span>
					<button
						type="button"
						class="flex-none cursor-pointer text-dim2 hover:text-destructive disabled:cursor-not-allowed disabled:opacity-40"
						disabled={removingId === m.id}
						onclick={() => removeMember(m)}
						aria-label={`Remove ${parsed.venue}:${parsed.ticker} from ${group.name}`}
					>
						<XIcon class="size-3" />
					</button>
				</div>
			{/each}
			{#if members.length === 0}
				<p class="px-3 py-2 font-mono text-[9px] tracking-[0.16em] text-dim">EMPTY</p>
			{/if}
			{#if removeError}
				<p class="px-3 py-1 text-[10.5px] text-destructive">{removeError}</p>
			{/if}
			<div class="border-t border-ink/6 p-2">
				<Input
					bind:value={query}
					placeholder="Add instrument…"
					class="h-7 border-ink/14 bg-transparent font-mono text-[10px]"
				/>
				{#if results.length > 0}
					<div class="mt-1 max-h-[140px] overflow-auto border border-ink/10">
						{#each results as r (r.id)}
							<button
								type="button"
								class="flex w-full cursor-pointer items-center gap-2 px-2 py-1.5 text-left disabled:cursor-not-allowed disabled:opacity-40"
								disabled={addingId === r.id}
								onclick={() => addMember(r)}
							>
								<CoinsIcon class="size-3 flex-none text-dim2" />
								<span class="truncate font-mono text-[10px] text-foreground">{r.source_id.toUpperCase()}:{r.symbol}</span>
								<span class="flex-none truncate font-mono text-[8.5px] text-dim">{r.name}</span>
							</button>
						{/each}
					</div>
				{/if}
				{#if addError}
					<p class="pt-1 text-[10.5px] text-destructive">{addError}</p>
				{/if}
			</div>
		{/if}
	</Accordion.Content>
</Accordion.Item>
