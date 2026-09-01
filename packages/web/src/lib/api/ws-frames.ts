// Pure per-frame checks over `WsEvent` (`ws-events.svelte.ts`), kept in a
// plain `.ts` module the same way `priceFromEvent` conceptually is one —
// but `ws-events.svelte.ts` also constructs its `$state`-backed
// `wsEventsStore` singleton at module scope, so importing *that* file
// anywhere outside Svelte's own compiler (a plain `bun test`, notably)
// throws `$state is not defined` before a single assertion runs. `import
// type` erases at build time and never touches that singleton, so this
// file can depend on `WsEvent`'s shape without depending on the module that
// constructs it, and stay unit-testable with a plain `bun test`.
import type { WsEvent } from './ws-events.svelte';

/** `crates/api/src/ws.rs`'s `unsupported` frame — sent where `subscribed`
 * would otherwise go, for a topic whose source has no feed pool in this
 * build. This is the WS layer's own half of "does this venue stream a
 * price": `$lib/api/sources.svelte.ts`'s `GET /api/sources` cache is the
 * other, checked *before* subscribing; this is the authoritative per-topic
 * answer once a subscribe has actually gone out. */
interface WsUnsupportedPayload {
	topic: string;
}

interface WsQuotePayload {
	topic: string;
	bid: number;
	ask: number;
	price_scale: number;
}

function isQuotePayload(payload: unknown): payload is WsQuotePayload {
	if (typeof payload !== 'object' || payload === null) return false;
	const p = payload as Record<string, unknown>;
	return (
		typeof p.topic === 'string' &&
		typeof p.bid === 'number' &&
		typeof p.ask === 'number' &&
		typeof p.price_scale === 'number'
	);
}

function isUnsupportedPayload(payload: unknown): payload is WsUnsupportedPayload {
	if (typeof payload !== 'object' || payload === null) return false;
	return typeof (payload as Record<string, unknown>).topic === 'string';
}

/** Whether `event` is the `unsupported` frame naming `topic` specifically —
 * `false` for anything else (a different topic, a different frame type, or
 * no event at all yet), the same shape `priceFromEvent` already uses. */
export function isUnsupportedFor(event: WsEvent | null, topic: string): boolean {
	if (!event || event.type !== 'unsupported' || !isUnsupportedPayload(event.payload)) return false;
	return event.payload.topic === topic;
}

/** Converts a quote frame's scaled bid and ask only at the rendering edge.
 * A wrong topic or incomplete wire frame is not a quote for this pane. */
export function quoteFromEvent(event: WsEvent | null, topic: string): { bid: number; ask: number } | null {
	if (!event || event.type !== 'quote' || !isQuotePayload(event.payload)) return null;
	if (event.payload.topic !== topic) return null;
	const scale = 10 ** event.payload.price_scale;
	return { bid: event.payload.bid / scale, ask: event.payload.ask / scale };
}

/** `crates/api/src/ws.rs`'s `ServerFrame::Indicator` — one field of a live
 * indicator's latest reading, for `topic` (`$lib/api/indicator-topic.ts`'s
 * `indicatorTopic`). A compound indicator (Macd, Stochastic,
 * BollingerBands) sends one frame per field it reports, all under the same
 * `topic`, matching how `field_key`/`reported_fields` already name a
 * `POST /api/indicators/compute` response's per-field series. `value` is in
 * the same scaled units that response's own points are — a caller still
 * divides by the instrument's price/qty scale before drawing it. */
export interface WsIndicatorPayload {
	topic: string;
	field: string;
	value: number;
	/** The bar this reading was computed from, Unix nanoseconds — matching
	 * `BarDto`'s own convention, not `Price`'s milliseconds. */
	ts_open: number;
	/** `false` once the bar this reading covers has actually closed. A
	 * chart draws both; nothing that acts on a value (an alert, a backtest)
	 * may ever treat `true` as settled. */
	provisional: boolean;
}

function isIndicatorPayload(payload: unknown): payload is WsIndicatorPayload {
	if (typeof payload !== 'object' || payload === null) return false;
	const p = payload as Record<string, unknown>;
	return (
		typeof p.topic === 'string' &&
		typeof p.field === 'string' &&
		typeof p.value === 'number' &&
		typeof p.ts_open === 'number' &&
		typeof p.provisional === 'boolean'
	);
}

/** `event`'s indicator reading for `topic`, or `null` for anything else (a
 * different topic, a different frame type, or no event at all yet) — the
 * same shape `quoteFromEvent` already uses. */
export function indicatorFrameFor(event: WsEvent | null, topic: string): WsIndicatorPayload | null {
	if (!event || event.type !== 'indicator' || !isIndicatorPayload(event.payload)) return null;
	if (event.payload.topic !== topic) return null;
	return event.payload;
}

/** One resting order-book level on the wire — `crates/api/src/ws.rs`'s
 * `BookLevelWire`. Scaled by `WsBookPayload`'s shared `price_scale`/
 * `qty_scale`, the same scaled-integer pair `PriceUpdate`/`QuoteUpdate`
 * already carry — never converted to a float for anything but the
 * rendering edge. */
export interface WsBookLevel {
	price: number;
	size: number;
}

/** `crates/api/src/ws.rs`'s `ServerFrame::Book` — a fixed-depth snapshot for
 * `topic` (`book:<source>:<symbol>`), sent once per `subscribe`. There is no
 * continuous stream to fold into: a caller that wants a fresher snapshot
 * sends `unsubscribe` then `subscribe` again, the same "one request, one
 * frame" contract the server's own doc states. `bids`/`asks` arrive best
 * price first, exactly as the venue reported them. */
export interface WsBookPayload {
	topic: string;
	bids: WsBookLevel[];
	asks: WsBookLevel[];
	price_scale: number;
	qty_scale: number;
	ts: number;
}

function isBookLevel(level: unknown): level is WsBookLevel {
	if (typeof level !== 'object' || level === null) return false;
	const l = level as Record<string, unknown>;
	return typeof l.price === 'number' && typeof l.size === 'number';
}

function isBookPayload(payload: unknown): payload is WsBookPayload {
	if (typeof payload !== 'object' || payload === null) return false;
	const p = payload as Record<string, unknown>;
	return (
		typeof p.topic === 'string' &&
		Array.isArray(p.bids) &&
		p.bids.every(isBookLevel) &&
		Array.isArray(p.asks) &&
		p.asks.every(isBookLevel) &&
		typeof p.price_scale === 'number' &&
		typeof p.qty_scale === 'number'
	);
}

/** `event`'s book snapshot for `topic`, or `null` for anything else (a
 * different topic, a different frame type, an incomplete wire frame, or no
 * event at all yet) — the same shape `quoteFromEvent`/`indicatorFrameFor`
 * already use. Levels are returned as scaled integers; a caller divides by
 * `price_scale`/`qty_scale` only where it actually renders a number. */
export function bookFrameFor(event: WsEvent | null, topic: string): WsBookPayload | null {
	if (!event || event.type !== 'book' || !isBookPayload(event.payload)) return null;
	if (event.payload.topic !== topic) return null;
	return event.payload;
}
