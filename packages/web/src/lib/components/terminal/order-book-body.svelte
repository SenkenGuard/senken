<script lang="ts">
	// The order-book panel's honest states, as a plain, dependency-free
	// component (no websocket, no `$effect`, no `onMount`) — split out of
	// `order-book.svelte` the same way `history-edge-status.svelte` is split
	// out of `chart-pane.svelte`, so a test can render it directly and read
	// real `data-*` attributes off the output (`order-book-body.test.ts`).
	//
	// Six distinct states, never collapsed into one another: a source that
	// does not report `book.supported` is not the same fact as a snapshot
	// that has not arrived yet, and neither is the same as a snapshot that
	// arrived and is genuinely empty. One indistinguishable blank list for
	// three different facts is the honesty gap this exists to close
	// (`data-book-state` carries which one it is, not just whether rows are
	// showing).
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
		/** The server tried and could not. A snapshot that failed used to be
		 * indistinguishable from one still in flight, so the panel waited
		 * forever for a frame that was never coming. */
		failedTopic,
		/** `null` until this topic's first `book` frame has arrived; `[]` for a
		 * snapshot that arrived and is genuinely empty on both sides. */
		rows,
		/** The venue's own timestamp on the snapshot `rows` came from, already
		 * formatted (`formatBookTime`) — `null` when there is none to show. */
		updatedAt,
		onRetry
	}: {
		capability: boolean | undefined;
		unsupportedTopic: boolean;
		failedTopic: boolean;
		rows: BookRow[] | null;
		updatedAt: string | null;
		onRetry: () => void;
	} = $props();

	const state = $derived.by(
		(): 'unsupported' | 'failed' | 'checking' | 'waiting' | 'empty' | 'ready' => {
		if (capability === false || unsupportedTopic) return 'unsupported';
		if (failedTopic) return 'failed';
		if (capability === undefined) return 'checking';
		if (rows === null) return 'waiting';
		if (rows.length === 0) return 'empty';
		return 'ready';
		}
	);
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
	{:else if state === 'failed'}
		<!-- Stated, and retryable. The venue supports depth; this attempt did
		     not arrive, which is a different fact from both "not supported" and
		     "still waiting" — and the only one of the three worth a button. -->
		<div class="flex flex-col items-center gap-2 px-3 py-6">
			<span class="text-center font-mono text-[9px] tracking-[0.14em] text-dim">COULD NOT LOAD ORDER-BOOK DEPTH</span>
			<button
				type="button"
				onclick={onRetry}
				class="flex cursor-pointer items-center gap-1 font-mono text-[8px] tracking-[0.18em] text-dim2 hover:text-foreground"
				aria-label="Retry order book"
			>
				<RotateCcwIcon class="size-2.5" />
				RETRY
			</button>
		</div>
	{:else}
		<!-- No refresh control: depth refreshes itself, and a button offering
		     to do what is already happening tells the reader the opposite.
		     What replaces it is the one thing they cannot otherwise know —
		     when this ladder was reported. A time that stops advancing is the
		     honest signal that the stream has stopped; a "LIVE" badge would
		     go on claiming otherwise. -->
		<div class="flex items-center justify-end border-b border-ink/6 px-3 py-1">
			<span data-book-updated-at={updatedAt} class="font-mono text-[8px] tracking-[0.18em] text-dim2">
				{updatedAt ? `UPDATED ${updatedAt}` : 'UPDATED —'}
			</span>
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
