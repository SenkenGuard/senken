// Where WebSocket messages land ("its events are published to a store; components never hold a socket"). `websocket.ts` is the only
// writer; every component reads `wsEventsStore.last` reactively instead of
// importing `WsClient` or touching a `WebSocket` itself — same one-funnel
// rule as `ApiClient` for REST.
//
// The envelope itself (`type`/`payload`/`receivedAt`) stays generic: plan
// 006 S5 only defines one real message shape a component actually needs to
// read (`price`, `crates/api/src/ws.rs`'s `ServerFrame::Price`) — narrowing
// `payload` itself into a full discriminated union for `connected`/
// `subscribed`/`unsubscribed` too would be speculative typing for messages
// nothing here reads yet. `priceFromEvent` is that one real shape, kept
// here (not duplicated per call site) so every reader of a price tick
// agrees on the wire fields.

export interface WsEvent {
	type: string;
	payload: unknown;
	receivedAt: number;
}

/** `crates/api/src/ws.rs`'s `ServerFrame::Price` — one live tick for
 * `topic` (an instrument's `source:symbol` wire form, the same string a
 * caller passed to `wsClient.subscribe`). `price`/`price_scale` are the
 * scaled-integer pair every other endpoint in this app reports a price as
 * (never a bare float over the wire); `toRealPrice` is the one place that
 * turns the pair into the decimal a component actually renders. */
interface WsPricePayload {
	topic: string;
	price: number;
	price_scale: number;
	/** Base-asset quantity traded at `price`, the same scaled-integer pair
	 * treatment — a bar is OHLCV, and the V has to reach the client too. */
	qty: number;
	qty_scale: number;
	ts: number;
}

function isPricePayload(payload: unknown): payload is WsPricePayload {
	if (typeof payload !== 'object' || payload === null) return false;
	const p = payload as Record<string, unknown>;
	return typeof p.topic === 'string' && typeof p.price === 'number' && typeof p.price_scale === 'number';
}

/** The real decimal price from `event`, if it is a `price` frame for
 * `topic` — `null` for anything else (a different topic, a different frame
 * type, or no event at all yet). */
export function priceFromEvent(event: WsEvent | null, topic: string): number | null {
	if (!event || event.type !== 'price' || !isPricePayload(event.payload)) return null;
	if (event.payload.topic !== topic) return null;
	return event.payload.price / 10 ** event.payload.price_scale;
}

/** The traded quantity from the same frame, as a decimal. `null` under the
 * same conditions as `priceFromEvent`, and `0` for a venue that reports a
 * trade without a size — which is not the same thing as no trade. */
export function qtyFromEvent(event: WsEvent | null, topic: string): number | null {
	if (!event || event.type !== 'price' || !isPricePayload(event.payload)) return null;
	if (event.payload.topic !== topic) return null;
	if (typeof event.payload.qty !== 'number') return 0;
	return event.payload.qty / 10 ** (event.payload.qty_scale ?? 0);
}

class WsEventsStore {
	last = $state<WsEvent | null>(null);
}

export const wsEventsStore = new WsEventsStore();

export function publishWsEvent(event: WsEvent): void {
	wsEventsStore.last = event;
}
