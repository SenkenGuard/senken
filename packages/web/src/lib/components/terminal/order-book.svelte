<script lang="ts">
	// Owns the live order-book snapshot for `instrument`; the actual states
	// and markup are `order-book-body.svelte` (kept pure so a test can render
	// them without a websocket — see that file's own doc).
	//
	// A fixed-depth book (`crates/subscription::BookSource`) is one request,
	// one WS frame, never a continuous stream (`ws-frames.ts`'s
	// `bookFrameFor` doc) — so this subscribes on mount/instrument change,
	// applies whatever `book` frame answers for its own topic, and offers a
	// manual refresh (unsubscribe, then subscribe again) rather than polling.
	// A source that does not report `book.supported` never subscribes at all
	// — the same "no silent no-op" contract `hasQuoteFeed`'s bid/ask lines
	// already follow (`chart-settings.ts`'s `disabledReason`).
	import { wsClient } from '$lib/api/websocket';
	import { wsEventsStore } from '$lib/api/ws-events.svelte';
	import { bookFrameFor, isFailedFor,
	isUnsupportedFor, type WsBookPayload } from '$lib/api/ws-frames';
	import { hasBookFeed } from '$lib/api/sources.svelte';
	import { bookRowsFromSnapshot } from './order-book';
	import OrderBookBody from './order-book-body.svelte';

	let { instrument }: { instrument: string } = $props();

	const topic = $derived(`book:${instrument}`);
	/** `undefined` while `GET /api/sources` has not resolved yet — never read
	 * as "supported" on the strength of an absent answer. */
	const supported = $derived(hasBookFeed(instrument));

	let snapshot = $state<WsBookPayload | null>(null);
	/** The authoritative "this venue answered unsupported" fact, set once a
	 * `subscribe` actually goes out and the server answers — distinct from
	 * `supported === false`, which is the capability response's own,
	 * pre-subscribe answer to the same question. */
	let unsupportedTopic = $state(false);
	/** The server tried and could not — cleared on every fresh subscribe,
	 * so a retry starts from "not told otherwise" rather than from the
	 * previous attempt's failure. */
	let failedTopic = $state(false);

	$effect(() => {
		const t = topic;
		if (supported !== true) {
			snapshot = null;
			unsupportedTopic = false;
			failedTopic = false;
			return;
		}
		wsClient.subscribe(t);
		snapshot = null;
		unsupportedTopic = false;
		failedTopic = false;
		return () => wsClient.unsubscribe(t);
	});

	$effect(() => {
		const event = wsEventsStore.last;
		const t = topic;
		if (isUnsupportedFor(event, t)) {
			unsupportedTopic = true;
			return;
		}
		if (isFailedFor(event, t)) {
			failedTopic = true;
			return;
		}
		const frame = bookFrameFor(event, t);
		if (frame) snapshot = frame;
	});

	/** Asks for a fresh snapshot the only way the protocol allows one: an
	 * explicit `unsubscribe` followed by `subscribe` — there is nothing to
	 * "update" in between (`ws-frames.ts`'s own doc on `WsBookPayload`). */
	function refresh(): void {
		if (supported !== true) return;
		wsClient.unsubscribe(topic);
		wsClient.subscribe(topic);
	}

	const rows = $derived(snapshot ? bookRowsFromSnapshot(snapshot.bids, snapshot.asks, snapshot.price_scale, snapshot.qty_scale) : null);
</script>

<OrderBookBody capability={supported} {unsupportedTopic} {failedTopic} {rows} onRefresh={refresh} />
