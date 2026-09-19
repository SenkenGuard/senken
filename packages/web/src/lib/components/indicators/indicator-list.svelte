<script lang="ts">
	// The dock's left column — "MY INDICATORS", Pine Editor's own script
	// list simplified to what this MVP needs: a status dot, a title, and a
	// row per indicator. Purely presentational; `indicator-dock.svelte` owns
	// the store and passes down what to render.
	import { cn } from '$lib/utils.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import type { UserIndicatorSummaryDto } from '$lib/api/types';
	import SearchIcon from '@lucide/svelte/icons/search';
	import PlusIcon from '@lucide/svelte/icons/plus';

	let {
		items,
		activeId,
		loading,
		onSelect,
		onNew
	}: {
		items: UserIndicatorSummaryDto[];
		activeId: string | null;
		loading: boolean;
		onSelect: (id: string) => void;
		onNew: () => void;
	} = $props();

	let query = $state('');
	const filtered = $derived(
		query.trim() ? items.filter((i) => i.title.toLowerCase().includes(query.trim().toLowerCase())) : items
	);

	function statusOf(item: UserIndicatorSummaryDto): 'compiled' | 'error' | 'pending' {
		if (item.compiled) return 'compiled';
		if (item.compile_error) return 'error';
		return 'pending';
	}
</script>

<div data-indicator-list class="flex min-h-0 w-[200px] flex-none flex-col border-r border-ink/10">
	<div class="flex flex-none items-center justify-between gap-2 border-b border-ink/10 px-2.5 py-2">
		<span class="font-mono text-[8px] tracking-[0.24em] text-dim">MY INDICATORS</span>
		<button
			type="button"
			class="flex size-7 flex-none cursor-pointer items-center justify-center border border-ink/14 text-dim2 hover:text-foreground"
			onclick={onNew}
			aria-label="New indicator"
			title="New indicator"
		>
			<PlusIcon class="size-3" />
		</button>
	</div>

	{#if items.length > 8}
		<div class="flex flex-none items-center gap-1.5 border-b border-ink/8 px-2.5 py-1.5">
			<SearchIcon class="size-3 flex-none text-dim2" />
			<Input
				bind:value={query}
				placeholder="Search…"
				aria-label="Search my indicators"
				class="h-6 flex-1 rounded-none border-none bg-transparent px-0 text-[10.5px] shadow-none focus-visible:ring-0"
			/>
		</div>
	{/if}

	<div class="min-h-0 flex-1 overflow-auto">
		{#if loading && items.length === 0}
			<div class="flex flex-col gap-1.5 p-2.5">
				{#each Array.from({ length: 3 }) as _, i (i)}
					<div class="h-6 animate-pulse bg-ink/6"></div>
				{/each}
			</div>
		{:else if items.length === 0}
			<div class="flex flex-col items-center gap-2 px-3 py-6 text-center">
				<span class="font-mono text-[10px] tracking-[0.06em] text-dim2">No indicators yet.</span>
				<button
					type="button"
					class="cursor-pointer border border-ink/14 px-2.5 py-1 font-mono text-[9px] tracking-[0.14em] text-foreground hover:bg-ink/7"
					onclick={onNew}
				>
					CREATE ONE
				</button>
			</div>
		{:else if filtered.length === 0}
			<div class="px-3 py-6 text-center font-mono text-[10px] tracking-[0.06em] text-dim2">No match.</div>
		{:else}
			{#each filtered as item (item.id)}
				{@const status = statusOf(item)}
				{@const active = item.id === activeId}
				<button
					type="button"
					data-indicator-row
					data-status={status}
					class={cn(
						'flex w-full cursor-pointer items-center gap-2 border-b border-ink/5 px-2.5 py-2 text-left',
						active ? 'bg-foreground text-inv' : 'text-foreground hover:bg-ink/7'
					)}
					onclick={() => onSelect(item.id)}
				>
					<span
						class={cn(
							'size-[7px] flex-none rounded-full',
							status === 'compiled' ? 'bg-gain' : status === 'error' ? 'bg-loss' : 'bg-dim'
						)}
						aria-label={status === 'compiled' ? 'Compiled' : status === 'error' ? 'Build failed' : 'Not compiled yet'}
						title={status === 'compiled' ? 'Compiled' : status === 'error' ? 'Build failed' : 'Not compiled yet'}
					></span>
					<span class="truncate font-mono text-[11px] tracking-[0.03em]">{item.title}</span>
				</button>
			{/each}
		{/if}
	</div>
</div>
