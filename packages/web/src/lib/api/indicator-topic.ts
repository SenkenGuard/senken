// The client-outgoing half of a live indicator subscription
// (`crates/api/src/ws.rs`'s `ClientFrame::SubscribeIndicator`) — the
// request shape and the topic string it resolves to.
//
// Kept as its own pure, framework-independent module (the same reasoning
// `topic-refcount.ts` and `ws-frames.ts` already follow) so `indicatorTopic`
// is unit-testable with a plain `bun test`, without dragging in
// `websocket.ts`'s `$state`-backed singletons.

/** The request `websocket.ts`'s `WsClient.subscribeIndicator` sends as a
 * `subscribe_indicator` frame — matching `POST /api/indicators/compute`'s
 * own `instrument`/`spec`/`indicator`/`params`/`from`/`to` fields, so a
 * caller can reuse exactly what it already built for `loadIndicatorSeries`. */
export interface IndicatorSubscribeRequest {
	/** `source:symbol`, matching a price/quote topic's own convention. */
	instrument: string;
	/** The bar timeframe, e.g. `"1h"`. */
	spec: string;
	/** The indicator's name — see `GET /api/indicators`'s catalogue. */
	indicator: string;
	/** The indicator's parameters, as JSON-object text — the same string
	 * `loadIndicatorSeries` already sends to `POST /api/indicators/compute`. */
	params: string;
	/** Inclusive start of the warm-up range, Unix nanoseconds. */
	from: number;
	/** Exclusive end of the warm-up range, Unix nanoseconds. */
	to: number;
}

/** The canonical topic a `subscribe_indicator` request resolves to —
 * `indicator:<instrument>:<spec>:<indicator>:<params>`, namespaced so it
 * cannot be confused with the last-trade/quote streams for the same
 * instrument. Plain concatenation of the request's own raw fields, matching
 * `IndicatorSubscribeRequest::topic()` field-for-field — not a
 * canonicalized form, so this is exactly what the server's own `subscribed`
 * reply echoes back for the same request. */
export function indicatorTopic(
	request: Pick<IndicatorSubscribeRequest, 'instrument' | 'spec' | 'indicator' | 'params'>
): string {
	return `indicator:${request.instrument}:${request.spec}:${request.indicator}:${request.params}`;
}
