<script lang="ts">
	// One shown note inside `notes-tab.svelte`: its own title/body, loaded by
	// id (`listNotes`'s own doc — a listing row never carries a note's body,
	// only `getNote` does) and saved back through `apiClient.updateNote`.
	//
	// Every edit is debounced (~600ms of no further typing) and flushed
	// immediately on blur, so leaving the field or pausing both save without
	// ever writing on each keystroke. `saving` only shows while a request is
	// actually in flight — there is deliberately no "Saved" state, since
	// showing one the instant a keystroke lands would claim a save that has
	// not happened yet.
	import { onDestroy, onMount } from 'svelte';
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Textarea } from '$lib/components/ui/textarea/index.js';

	const SAVE_DEBOUNCE_MS = 600;

	let { noteId }: { noteId: string } = $props();

	let title = $state('');
	let body = $state('');
	let loading = $state(true);
	let loadError = $state<string | null>(null);
	let saving = $state(false);
	let saveError = $state<string | null>(null);

	let saveTimer: ReturnType<typeof setTimeout> | undefined;

	async function loadNote(): Promise<void> {
		loading = true;
		loadError = null;
		try {
			const note = await apiClient.getNote(noteId);
			title = note.title;
			body = note.body;
		} catch (error) {
			loadError = getErrorMessage(error, 'Could not load this note.');
		} finally {
			loading = false;
		}
	}

	onMount(() => void loadNote());

	async function flushSave(): Promise<void> {
		if (saveTimer) {
			clearTimeout(saveTimer);
			saveTimer = undefined;
		}
		saving = true;
		saveError = null;
		try {
			await apiClient.updateNote(noteId, { title, body });
		} catch (error) {
			saveError = getErrorMessage(error, 'Could not save this note.');
		} finally {
			saving = false;
		}
	}

	function scheduleSave(): void {
		if (saveTimer) clearTimeout(saveTimer);
		saveTimer = setTimeout(() => void flushSave(), SAVE_DEBOUNCE_MS);
	}

	onDestroy(() => {
		// A pending debounce must not vanish along with the component (the
		// note's own workspace being hidden, say, right after a keystroke) —
		// flush whatever is still unsaved rather than losing it silently.
		if (saveTimer) void flushSave();
	});
</script>

<div class="flex flex-col gap-1.5 border border-ink/10 bg-card2 p-2.5">
	{#if loading}
		<p class="font-mono text-[9px] tracking-[0.16em] text-dim">LOADING…</p>
	{:else if loadError}
		<p class="text-[10.5px] text-destructive">{loadError}</p>
	{:else}
		<Input
			bind:value={title}
			oninput={scheduleSave}
			onblur={flushSave}
			placeholder="Untitled note"
			class="h-7 border-transparent bg-transparent px-0 font-mono text-[11px] font-medium tracking-[0.04em] text-foreground focus-visible:border-ink/14 focus-visible:px-2"
		/>
		<Textarea
			bind:value={body}
			oninput={scheduleSave}
			onblur={flushSave}
			placeholder="Write a note…"
			class="min-h-24 resize-y rounded-none border-ink/12 bg-transparent font-mono text-[10.5px] leading-relaxed"
		/>
		<div class="flex h-3 items-center">
			{#if saving}
				<span class="font-mono text-[8.5px] tracking-[0.16em] text-dim">SAVING…</span>
			{:else if saveError}
				<span class="text-[10.5px] text-destructive">{saveError}</span>
			{/if}
		</div>
	{/if}
</div>
