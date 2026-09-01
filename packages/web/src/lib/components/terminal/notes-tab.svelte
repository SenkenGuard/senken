<script lang="ts">
	// The right panel's NOTES tab, replacing the old hardcoded single-string
	// placeholder (`+page.svelte`'s former `notes` branch, `ACTIVE_NOTE`/
	// `NOTE_TAGS` in `terminal/trade-demo.ts`). Notes are global per user
	// (`senken_notes::NoteStore`, `$lib/api/client.ts`'s own doc); which of
	// them show *in this workspace* is local, stored the same way the
	// WATCHLIST tab's shown groups are — in the workspace's own `settings`
	// text (`$lib/charts/workspace-settings.ts`).
	//
	// This component owns the note list and the NOTES popover that creates/
	// deletes notes and toggles which are shown. A note's title is edited in
	// its own body, not here — `note-editor.svelte` owns one shown note's
	// title/body and its own debounced save.
	import { onMount } from 'svelte';
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import { chartWorkspaceStore, updateWorkspaceSettings } from '$lib/charts/workspace-store.svelte';
	import { resolveShown } from '$lib/charts/workspace-settings';
	import * as Popover from '$lib/components/ui/popover/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import { Switch } from '$lib/components/ui/switch/index.js';
	import NoteEditor from './note-editor.svelte';
	import type { NoteSummaryDto } from '$lib/api/types';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';

	const flyoutClass = 'rounded-none border border-ink/16 bg-popover px-2.5 py-1.5 text-left shadow-[0_12px_28px_rgba(0,0,0,0.6)]';

	let notes = $state<NoteSummaryDto[]>([]);
	let notesLoading = $state(true);
	let notesError = $state<string | null>(null);

	async function loadNotes(): Promise<void> {
		notesLoading = true;
		notesError = null;
		try {
			const page = await apiClient.listNotes(200, 0);
			notes = page.rows;
		} catch (error) {
			notesError = getErrorMessage(error, 'Could not load your notes.');
		} finally {
			notesLoading = false;
		}
	}

	onMount(() => void loadNotes());

	const shownIds = $derived(chartWorkspaceStore.workspaceSettings.notes);
	const shownNotes = $derived(resolveShown(notes, shownIds));

	let notesMenuOpen = $state(false);
	let creatingNote = $state(false);
	let createNoteError = $state<string | null>(null);

	/** A blank note this workspace does not show by default — creating one
	 * only adds it to the user's own note list; `toggleShownNote` is the one
	 * action that shows it anywhere, the same as a freshly created watchlist
	 * group is not auto-shown either. */
	async function submitCreateNote(): Promise<void> {
		creatingNote = true;
		createNoteError = null;
		try {
			await apiClient.createNote({ title: 'Untitled note', body: '' });
			await loadNotes();
		} catch (error) {
			createNoteError = getErrorMessage(error, 'Could not create a new note.');
		} finally {
			creatingNote = false;
		}
	}

	let deleteNoteError = $state<string | null>(null);

	/** Deletes a note and, like `watchlist-tab.svelte`'s `deleteGroup`, drops
	 * its id from this workspace's own shown list so it does not sit there
	 * forever as an id nothing owns any more. */
	async function deleteNote(id: string): Promise<void> {
		deleteNoteError = null;
		try {
			await apiClient.deleteNote(id);
			await loadNotes();
			if (shownIds.includes(id)) {
				await updateWorkspaceSettings({ notes: shownIds.filter((nid) => nid !== id) });
			}
		} catch (error) {
			deleteNoteError = getErrorMessage(error, 'Could not delete that note.');
		}
	}

	function toggleShownNote(id: string): void {
		const next = shownIds.includes(id) ? shownIds.filter((nid) => nid !== id) : [...shownIds, id];
		void updateWorkspaceSettings({ notes: next });
	}
</script>

<div class="flex min-h-0 flex-1 flex-col">
	<div class="flex flex-none items-center justify-between gap-2 border-b border-ink/7 px-3 py-2">
		<span class="font-mono text-[8px] tracking-[0.26em] text-dim">SHOWN NOTES</span>
		<Tooltip.Provider>
			<Popover.Root bind:open={notesMenuOpen}>
				<Tooltip.Root>
					<Tooltip.Trigger>
						{#snippet child({ props })}
							<span {...props} class="contents">
								<Popover.Trigger
									class="flex h-6 cursor-pointer items-center gap-1.5 border border-ink/14 px-2"
								>
									<span class="font-mono text-[9px] tracking-[0.14em] text-dim2">NOTES</span>
									<ChevronDownIcon class="size-3 text-dim2" />
								</Popover.Trigger>
							</span>
						{/snippet}
					</Tooltip.Trigger>
					<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class={flyoutClass}>
						<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">MANAGE NOTES</span>
					</Tooltip.Content>
				</Tooltip.Root>
				<Popover.Content align="end" sideOffset={10} class="w-[280px] rounded-none border-ink/16 bg-popover p-0">
					<div class="border-b border-ink/7 px-3 py-2.5">
						<Tooltip.Root>
							<Tooltip.Trigger
								type="button"
								class="flex h-7 w-full cursor-pointer items-center justify-center gap-1.5 border border-dashed border-ink/18 text-dim2 hover:border-ink/30 hover:text-foreground disabled:cursor-not-allowed disabled:opacity-40"
								disabled={creatingNote}
								onclick={submitCreateNote}
							>
								<PlusIcon class="size-3" />
								<span class="font-mono text-[9px] tracking-[0.16em]">NEW NOTE</span>
							</Tooltip.Trigger>
							<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class={flyoutClass}>
								<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">CREATE NOTE</span>
							</Tooltip.Content>
						</Tooltip.Root>
					</div>
					{#if createNoteError}
						<p class="px-3 pb-2 text-[10.5px] text-destructive">{createNoteError}</p>
					{/if}
					<div class="px-3 pt-2.5 pb-1.5 font-mono text-[8px] tracking-[0.26em] text-dim">YOUR NOTES</div>
					{#if notesLoading}
						<p class="px-3 pb-2.5 font-mono text-[9px] tracking-[0.16em] text-dim">LOADING…</p>
					{:else if notes.length === 0}
						<p class="px-3 pb-2.5 font-mono text-[9px] tracking-[0.16em] text-dim">NO NOTES YET</p>
					{/if}
					{#each notes as n (n.id)}
						<div class="mx-1.5 mb-1 flex items-center gap-1.5 border border-transparent px-2 py-1.5">
							<Switch
								size="sm"
								checked={shownIds.includes(n.id)}
								onCheckedChange={() => toggleShownNote(n.id)}
								aria-label={`Show ${n.title || 'Untitled note'} in this workspace`}
							/>
							<span class="min-w-0 flex-1 truncate font-mono text-[11px] text-foreground">{n.title || 'Untitled note'}</span>
							<Tooltip.Root>
								<Tooltip.Trigger
									class="flex-none cursor-pointer text-dim2 hover:text-destructive"
									onclick={() => deleteNote(n.id)}
									aria-label={`Delete ${n.title || 'Untitled note'}`}
								>
									<Trash2Icon class="size-3" />
								</Tooltip.Trigger>
								<Tooltip.Content side="bottom" sideOffset={8} arrowClasses="hidden" class={flyoutClass}>
									<span class="font-mono text-[9px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">DELETE NOTE</span>
								</Tooltip.Content>
							</Tooltip.Root>
						</div>
					{/each}
					{#if deleteNoteError}
						<p class="px-3 pb-2 text-[10.5px] text-destructive">{deleteNoteError}</p>
					{/if}
				</Popover.Content>
			</Popover.Root>
		</Tooltip.Provider>
	</div>

	<div class="min-h-0 flex-1 overflow-auto">
		{#if notesLoading}
			<p class="py-5.5 text-center font-mono text-[9px] tracking-[0.16em] text-dim">LOADING NOTES…</p>
		{:else if notesError}
			<div class="m-3 border border-destructive/30 bg-destructive/10 px-2.5 py-2 text-destructive">
				<p class="text-[11px]">{notesError}</p>
			</div>
		{:else if shownNotes.length === 0}
			<div class="px-4 py-6 text-center">
				<p class="font-mono text-[9.5px] leading-relaxed text-dim">
					No notes shown in this workspace — pick one from Notes, or create one.
				</p>
			</div>
		{:else}
			<div class="flex flex-col gap-2.5 p-3">
				{#each shownNotes as n (n.id)}
					<NoteEditor noteId={n.id} />
				{/each}
			</div>
		{/if}
	</div>
</div>
