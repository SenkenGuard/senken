<script lang="ts">
	// The order-book panel's honest states, as a plain, dependency-free
	// component (no websocket, no `$effect`, no `onMount`) — split out of
	// `order-book.svelte` the same way `history-edge-status.svelte` is split
	// out of `chart-pane.svelte`, so a test can render it directly and read
	// real `data-*` attributes off the output (`order-book-body.test.ts`).
	//
	// Five distinct states, never collapsed into one another: a source that
	// does not report `book.supported` is not the same fact as a snapshot
	// that has not arrived yet, and neither is the same as a snapshot that
	// arrived and is genuinely empty — an indistinguishable blank list is
	// exactly the honesty gap 019 M3 exists to close (`data-book-state`
	// carries which one this is, not just whether rows are showing).
	import { cn } from '$lib/utils.js';
	import type { BookRow } from './order-book';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';

	let {
		/** `GET /api/sources`' own answer for this instrument's source —
		 * `undefined` while it has not resolved yet. */
		capability,
		/** The WS layer's own `unsupported` answer, once a `subscribe` has
		 * actually gone out for this topic — see `order-book.svelte`'s doc on
		 * why this is tracked separately from `capability`. */
		unsupportedTopic,
		/** `null` until this topic's first `book` frame has arrived; `[]` for a
		 * snapshot that arrived and is genuinely empty on both sides. */
		rows,
		onRefresh
	}: {
		capability: boolean | undefined;
		unsupportedTopic: boolean;
		rows: BookRow[] | null;
		onRefresh: () => void;
	} = $props();

	const state = $derived.by((): 'unsupported' | 'checking' | 'waiting' | 'empty' | 'ready' => {
		if (capability === false || unsupportedTopic) return 'unsupported';
		if (capability === undefined) return 'checking';
		if (rows === null) return 'waiting';
		if (rows.length === 0) return 'empty';
		return 'ready';
	});
</script>

<div data-book-state={state}>
	{#if state === 'unsupported'}
		<div class="px-3 py-6 text-center font-mono text-[9px] tracking-[0.14em] text-dim">
			THIS SOURCE DOES NOT REPORT ORDER-BOOK DEPTH
		</div>
	{:else if state === 'checking'}
		<div class="px-3 py-6 text-center font-mono text-[9px] tracking-[0.14em] text-dim">CHECKING ORDER-BOOK SUPPORT…</div>
	{:else if state === 'waiting'}
		<div class="px-3 py-6 text-center font-mono text-[9px] tracking-[0.14em] text-dim">WAITING FOR DEPTH SNAPSHOT…</div>
	{:else}
		<div class="flex items-center justify-end border-b border-ink/6 px-3 py-1">
			<button
				type="button"
				onclick={onRefresh}
				class="flex cursor-pointer items-center gap-1 font-mono text-[8px] tracking-[0.18em] text-dim2 hover:text-foreground"
				aria-label="Refresh order book"
			>
				<RotateCcwIcon class="size-2.5" />
				REFRESH
			</button>
		</div>
		{#if state === 'empty'}
			<div class="px-3 py-6 text-center font-mono text-[9px] tracking-[0.14em] text-dim">ORDER BOOK IS EMPTY</div>
		{:else}
			{#each rows ?? [] as row, i (i)}
				<div class="relative flex items-center justify-between px-3 py-1 font-mono text-[10px]">
					<div
						class={cn('absolute inset-y-0 right-0', row.side === 'ask' ? 'bg-loss/10' : 'bg-gain/10')}
						style="width: {row.depthPct}%;"
					></div>
					<span class={cn('relative', row.side === 'ask' ? 'text-loss' : 'text-gain')}>{row.price}</span>
					<span class="relative text-secondary-foreground">{row.size}</span>
				</div>
			{/each}
		{/if}
	{/if}
</div>
