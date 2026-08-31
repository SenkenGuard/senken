<script lang="ts">
	// Left navigation + the search field above it. Search
	// filters *across every registered section* — its stated reason is
	// that "a registry-driven section list grows unpredictably once
	// plugins add to it," so browsing the list alone stops being enough.
	//
	// While a query is active, the nav narrows to sections that actually
	// contain a match (rather than hiding the rest outright — a user
	// mid-search can still see where they are) and a flat list of matching
	// rows appears beneath it; picking one jumps straight to that row in
	// its section (`jumpToSettingsRow`) instead of making the user re-find
	// it by browsing.
	import { onMount } from 'svelte';
	import { cn } from '$lib/utils.js';
	import SearchIcon from '@lucide/svelte/icons/search';
	import { listSettingsSections, searchSettings } from '$lib/state/settings-registry.svelte';
	import { settingsModal, selectSettingsSection, jumpToSettingsRow } from '$lib/state/settings.svelte';
	import { accessVisibility, loadAccessVisibility, canSeeAccessSection } from './access-visibility.svelte';

	// `section.adminOnly` now does filter this list, as of
	// Q10.2: `GET /api/me`'s `roles`/`grants` fields finally
	// give the client an honest signal to hide on, via
	// `access-visibility.svelte.ts`. This remains purely cosmetic — the
	// actual requirement, that every endpoint re-checks a real grant on
	// every request, is unaffected by whatever this list shows or hides.
	onMount(loadAccessVisibility);

	const sections = $derived(
		listSettingsSections().filter(
			(section) => !section.adminOnly || canSeeAccessSection(accessVisibility.profile)
		)
	);
	const query = $derived(settingsModal.query);
	const results = $derived(searchSettings(query));
	const matchedSectionIds = $derived(new Set(results.map((r) => r.sectionId)));
	const isSearching = $derived(query.trim() !== '');

	let inputEl = $state<HTMLInputElement | null>(null);

	export function focusSearch() {
		inputEl?.focus();
	}
</script>

<div class="flex h-full w-[220px] flex-none flex-col border-r border-ink/10 bg-card2">
	<div class="flex flex-none items-center gap-2 border-b border-ink/9 px-3.5 py-3">
		<SearchIcon class="size-[13px] flex-none text-dim2" />
		<input
			bind:this={inputEl}
			bind:value={settingsModal.query}
			placeholder="Search settings…"
			class="min-w-0 flex-1 bg-transparent text-[12.5px] text-foreground outline-none placeholder:text-dim"
		/>
	</div>

	<nav class="flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto py-2">
		{#each sections as section (section.id)}
			{@const active = section.id === settingsModal.activeSectionId}
			{@const dimmed = isSearching && !matchedSectionIds.has(section.id)}
			<button
				type="button"
				onclick={() => selectSettingsSection(section.id)}
				class={cn(
					'mx-2 flex cursor-pointer items-center gap-2.5 px-2.5 py-2 text-left transition-colors',
					active ? 'bg-ink/9 text-foreground' : 'text-secondary-foreground hover:bg-ink/6',
					dimmed && !active ? 'opacity-40' : ''
				)}
			>
				<section.icon class="size-[15px] flex-none" />
				<span class="min-w-0 flex-1 truncate text-[12.5px]">{section.label}</span>
			</button>
		{/each}

		{#if isSearching}
			<div class="mt-2 flex flex-col gap-0.5 border-t border-ink/9 px-2 pt-2">
				<span class="px-1 pb-1 font-mono text-[8.5px] tracking-[0.18em] text-dim">
					{results.length} MATCH{results.length === 1 ? '' : 'ES'}
				</span>
				{#each results as result (result.sectionId + result.entry.rowId)}
					<button
						type="button"
						onclick={() => jumpToSettingsRow(result.sectionId, result.entry.rowId)}
						class="flex flex-col items-start gap-0 px-2.5 py-1.5 text-left hover:bg-ink/6"
					>
						<span class="truncate text-[12px] text-foreground">{result.entry.rowLabel}</span>
						<span class="truncate text-[10.5px] text-dim2">{result.sectionLabel} · {result.entry.groupHeading}</span>
					</button>
				{/each}
				{#if results.length === 0}
					<p class="px-2.5 py-2 text-[11.5px] text-dim">No matching setting.</p>
				{/if}
			</div>
		{/if}
	</nav>
</div>
