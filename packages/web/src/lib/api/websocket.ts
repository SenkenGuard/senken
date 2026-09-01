// The WebSocket client half. "Build the client half:
// obtain a ticket through `ApiClient`, connect, reconnect with backoff and
// jitter, re-subscribe on reconnect, and publish events to a store so
// components never hold a socket." The server endpoint does not exist
// yet — see the module doc below for how this is structured to be pointed
// at one, and how it was exercised without a
// server.
//
// A browser cannot set an `Authorization` header on a WebSocket handshake
// (its whole reason for existing), so the session token never goes near
// this file. Instead: request a single-use ticket over REST through
// `apiClient` (the one funnel), then present *that* on the
// handshake. Putting the ticket in the query string is safe specifically
// because B3 designed it to be — "valid for seconds" and single-use, so a
// leaked ticket is worthless by the time it surfaces in a log. This is not
// the same mistake as putting the real session token in the query string,
// which B3 rejects for exactly that reason.
import { apiClient } from './client';
import { activeServer, resolveBaseUrl } from './servers.svelte';
import { connectionStore, setConnectionState } from './connection.svelte';
import { describeError } from './errors';
import { backoffDelay } from './backoff';
import { publishWsEvent, type WsEvent } from './ws-events.svelte';
import { TopicRefCounter } from './topic-refcount';
import { indicatorTopic, type IndicatorSubscribeRequest } from './indicator-topic';
import type { WsTicketResponse } from './types';

// Q4 landed: `POST /api/ws/ticket` and `GET /api/ws` (`crates/api/src/lib.rs`'s
// router), matching the provisional paths this file guessed at before the
// server existed.
const WS_TICKET_PATH = '/api/ws/ticket';
const WS_PATH = '/api/ws';

async function requestTicket(): Promise<string> {
	const { ticket } = await apiClient.request<WsTicketResponse>(WS_TICKET_PATH, { method: 'POST' });
	return ticket;
}

function wsUrl(ticket: string): string {
	const base = resolveBaseUrl(activeServer());
	const url = new URL(WS_PATH, base);
	url.protocol = url.protocol === 'https:' ? 'wss:' : 'ws:';
	url.searchParams.set('ticket', ticket);
	return url.toString();
}

function parseWsMessage(data: unknown): WsEvent | null {
	if (typeof data !== 'string') return null;
	try {
		const parsed: unknown = JSON.parse(data);
		if (typeof parsed === 'object' && parsed !== null && 'type' in parsed) {
			return {
				type: String((parsed as Record<string, unknown>).type),
				payload: parsed,
				receivedAt: Date.now()
			};
		}
	} catch {
		// Not JSON. Q4 hasn't defined the wire format yet, so this is a
		// defensive parse, not a schema violation worth surfacing.
	}
	return null;
}

/**
 * Owns exactly one live `WebSocket` at a time. Reconnection here is
 * independent of `ApiClient.startHeartbeat`'s REST polling — a dropped
 * socket and a slow REST response are different failures — but both write
 * into the same shared `connectionStore` (one machine, read by "both
 * the UI and the WS layer"). With no real server-side
 * WS endpoint to hold a socket open, the two can occasionally race and
 * overwrite each other's transition (e.g. a REST heartbeat tick marking
 * `'authenticated'` moments after a WS drop marked `'reconnecting'`); this
 * is flagged in the implementation report as worth revisiting once the * real protocol exists to drive reconciliation instead of two independent
 * pollers.
 */
class WsClient {
	private socket: WebSocket | null = null;
	// Reference-counted (`TopicRefCounter`), not a plain `Set`:
	// puts more than one leaseholder on the same shared connection (a chart
	// pane and a watchlist row can easily name the same instrument),
	// mirroring `senken_subscription::SubscriptionPool`'s own
	// `LeaseRecord.count` — the server only unsubscribes a topic on its
	// *last* lease, so this client must not send a real `unsubscribe` frame
	// (which tears down the one shared per-connection lease,
	// `crates/api/src/ws.rs`'s own doc) on anything but the last local
	// caller's `unsubscribe` either.
	private readonly subscriptions = new TopicRefCounter();
	// The `subscribe_indicator` request behind each currently-referenced
	// indicator topic, so `open` (reconnect) can replay the right frame kind
	// for it below — a live indicator session lives inside the connection
	// that opened it (`crates/api/src/ws.rs`'s own per-connection
	// `subscriptions: HashMap<String, AbortHandle>`), unlike a price/quote
	// lease's topic string, which a plain `subscribe` frame is enough to
	// re-establish. Entries are added on the first reference to a topic and
	// removed on the last release, in step with `subscriptions` itself.
	private readonly indicatorRequests = new Map<string, IndicatorSubscribeRequest>();
	private attempt = 0;
	private reconnectTimer: ReturnType<typeof setTimeout> | null = null;
	private generation = 0;
	private stopped = true;

	/** Takes one more reference on `topic`, sending its subscribe frame only
	 * on the first. Every currently-referenced topic is replayed on future
	 * reconnect — B16: "re-subscribes on reconnect." */
	subscribe(topic: string): void {
		if (this.subscriptions.retain(topic)) this.sendFrame('subscribe', topic);
	}

	/** Opens (or, ref-counted the same way `subscribe` is, joins) a live
	 * indicator session and returns its canonical topic
	 * (`indicator-topic.ts`'s `indicatorTopic`) — computed the same way the
	 * server does, so the caller can key its own bookkeeping (which series
	 * to update on an incoming `indicator` frame) on it immediately, rather
	 * than waiting for the `subscribed` reply. Sends the `subscribe_indicator`
	 * frame only on the first outstanding reference for this exact topic;
	 * later callers for the same `(instrument, spec, indicator, params)`
	 * share it, released the same way `unsubscribe` already releases any
	 * other topic. */
	subscribeIndicator(request: IndicatorSubscribeRequest): string {
		const topic = indicatorTopic(request);
		if (this.subscriptions.retain(topic)) {
			this.indicatorRequests.set(topic, request);
			this.sendIndicatorFrame(request);
		}
		return topic;
	}

	/** Releases one reference on `topic`. Only the caller dropping the last
	 * outstanding reference actually sends `unsubscribe` — same "the last
	 * lease releases it" contract `SubscriptionPool::lease`/`Drop for Lease`
	 * implement server-side. Works uniformly for a price/quote topic or an
	 * indicator one: the server's own `Unsubscribe` handling only looks the
	 * topic string up in its per-connection map, regardless of which frame
	 * originally inserted it. */
	unsubscribe(topic: string): void {
		if (this.subscriptions.release(topic)) {
			this.indicatorRequests.delete(topic);
			this.sendFrame('unsubscribe', topic);
		}
	}

	connect(): void {
		this.stopped = false;
		this.attempt = 0;
		const generation = ++this.generation;
		void this.attemptConnect(generation);
	}

	disconnect(): void {
		this.stopped = true;
		this.generation++;
		if (this.reconnectTimer !== null) {
			clearTimeout(this.reconnectTimer);
			this.reconnectTimer = null;
		}
		this.socket?.close();
		this.socket = null;
	}

	private async attemptConnect(generation: number): Promise<void> {
		if (this.stopped || generation !== this.generation) return;
		setConnectionState(this.attempt === 0 ? 'connecting' : 'reconnecting');

		let ticket: string;
		try {
			ticket = await requestTicket();
		} catch (error) {
			this.scheduleReconnect(generation, error);
			return;
		}
		if (this.stopped || generation !== this.generation) return;

		const socket = new WebSocket(wsUrl(ticket));
		this.socket = socket;

		socket.addEventListener('open', () => {
			if (generation !== this.generation) return;
			this.attempt = 0;
			setConnectionState('authenticated');
			for (const topic of this.subscriptions.topics()) {
				const indicatorRequest = this.indicatorRequests.get(topic);
				if (indicatorRequest) this.sendIndicatorFrame(indicatorRequest);
				else this.sendFrame('subscribe', topic);
			}
		});

		socket.addEventListener('message', (event: MessageEvent) => {
			if (generation !== this.generation) return;
			const parsed = parseWsMessage(event.data as unknown);
			if (parsed) publishWsEvent(parsed);
		});

		socket.addEventListener('close', () => {
			if (this.socket !== socket) return; // superseded by a newer attempt
			this.socket = null;
			if (this.stopped || generation !== this.generation) return;
			this.scheduleReconnect(generation);
		});

		// A WebSocket always fires 'close' after 'error' (per the WHATWG
		// spec), so reconnect scheduling lives in the 'close' handler above;
		// this listener only exists so the error event isn't reported as an
		// unhandled one.
		socket.addEventListener('error', () => {});
	}

	private scheduleReconnect(generation: number, error?: unknown): void {
		if (this.stopped || generation !== this.generation) return;
		setConnectionState('reconnecting', error ? describeError(error) : connectionStore.lastError);
		const delay = backoffDelay(this.attempt++);
		this.reconnectTimer = setTimeout(() => void this.attemptConnect(generation), delay);
	}

	private sendFrame(type: 'subscribe' | 'unsubscribe', topic: string): void {
		if (this.socket?.readyState === WebSocket.OPEN) {
			this.socket.send(JSON.stringify({ type, topic }));
		}
	}

	private sendIndicatorFrame(request: IndicatorSubscribeRequest): void {
		if (this.socket?.readyState === WebSocket.OPEN) {
			this.socket.send(JSON.stringify({ type: 'subscribe_indicator', ...request }));
		}
	}
}

export const wsClient = new WsClient();
