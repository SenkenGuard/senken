<script lang="ts">
	// Owns the live order-book snapshot for `instrument`; the actual states
	// and markup are `order-book-body.svelte` (kept pure so a test can render
	// them without a websocket — see that file's own doc).
	//
	// Depth arrives as a stream of whole snapshots, not one snapshot: the
	// server refreshes the book on a cadence for as long as the topic is
	// subscribed (`senken_subscription::BookSessionRegistry`) and republishes
	// each one. So this subscribes on mount/instrument change and renders
	// whatever the newest `book` frame for its own topic says — there is
	// nothing for a reader to press, and no polling to do here: the loop that
	// keeps the book current runs once on the server, shared by every panel
	// watching that instrument, rather than once per open panel.
	//
	// A source that does not report `book.supported` never subscribes at all
	// — the same "no silent no-op" contract `hasQuoteFeed`'s bid/ask lines
	// already follow (`chart-settings.ts`'s `disabledReason`).
	import { untrack } from 'svelte';
	import { wsClient } from '$lib/api/websocket';
	import { wsEventsStore } from '$lib/api/ws-events.svelte';
	import { hasBookFeed } from '$lib/api/sources.svelte';
	import { bookRowsFromSnapshot, foldBookEvent, formatBookTime, NO_BOOK_YET } from './order-book';
	import OrderBookBody from './order-book-body.svelte';

	let { instrument }: { instrument: string } = $props();

	const topic = $derived(`book:${instrument}`);
	/** `undefined` while `GET /api/sources` has not resolved yet — never read
	 * as "supported" on the strength of an absent answer. */
	const supported = $derived(hasBookFeed(instrument));

	/** Everything heard about the current topic, folded by `foldBookEvent` —
	 * one value rather than three flags that could disagree about which
	 * topic they describe. */
	let book = $state({ ...NO_BOOK_YET });

	$effect(() => {
		const t = topic;
		// Reset before anything else: state left over from the previous
		// instrument would otherwise be read as this one's until its first
		// frame lands.
		book = { ...NO_BOOK_YET };
		if (supported !== true) return;
		wsClient.subscribe(t);
		return () => wsClient.unsubscribe(t);
	});

	$effect(() => {
		const event = wsEventsStore.last;
		const t = topic;
		// `untrack` because this effect writes the same state it folds from:
		// reading it as a dependency would re-run the effect on its own
		// write, forever.
		book = foldBookEvent(untrack(() => book), event, t);
	});

	/** Restarts this topic's subscription — an explicit `unsubscribe` followed
	 * by `subscribe`, which drops this connection's share of the server-side
	 * session and joins a fresh one, fetching straight away rather than at the
	 * next refresh.
	 *
	 * Only ever offered for a failure the server has already reported. The
	 * server's own loop keeps trying and recovers unprompted, so this is a
	 * shortcut past the wait, never the only way back — a reader who ignores
	 * it still gets their book. */
	function retry(): void {
		if (supported !== true) return;
		book = { ...NO_BOOK_YET };
		wsClient.unsubscribe(topic);
		wsClient.subscribe(topic);
	}

	const rows = $derived(
		book.snapshot
			? bookRowsFromSnapshot(book.snapshot.bids, book.snapshot.asks, book.snapshot.price_scale, book.snapshot.qty_scale)
			: null
	);
	const updatedAt = $derived(formatBookTime(book.snapshot?.ts));
</script>

<OrderBookBody
	capability={supported}
	unsupportedTopic={book.unsupported}
	failedTopic={book.failed}
	{rows}
	{updatedAt}
	onRetry={retry}
/>
