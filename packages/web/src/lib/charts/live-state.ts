// One pane's live-price state, as a discriminated union so the chart
// can never show a countdown and a "no feed" overlay at the same time — the
// two are read off the same `status` field, never independent booleans a
// caller could set inconsistently.
//
// A catalogued instrument status and a feed capability answer different
// questions. In particular, a closed instrument is not evidence that its
// venue lacks a feed. The optional reopen instant is carried only when a
// source supplied it; omitting it is more honest than inventing a schedule.

/** `#f2f2ef` — the same off-white this app already treats as its neutral
 * default (`chart-pane.svelte`'s `DEFAULT_DRAWING_STYLE.color`, a freshly
 * drawn object's starting colour, chosen for the identical reason: neither
 * green nor red, readable in both themes). Reused here rather than picking
 * a second "neutral" colour for the price line. */
export const NEUTRAL_PRICE_LINE_COLOR = '#f2f2ef';

export type LiveState =
	| { status: 'live' }
	| { status: 'no-feed' }
	| { status: 'closed'; opensAt?: number };

/** A source-supplied market state relevant to the chart's live overlay.
 * `opensAt` is Unix nanoseconds and is optional because an instrument
 * catalog can establish that a market is closed without promising a future
 * session time. */
export type MarketStatus = { status: 'closed'; opensAt?: number } | { status: 'trading' };

/** Combines `GET /api/sources`' registered-source fact with the WS layer's
 * own per-topic `unsupported` frame: either source alone leaves a gap.
 * `sourceHasLiveFeed` is `undefined` while `GET /api/sources` has not
 * resolved yet — treated the same as `true` so a pane is never shown a
 * "no feed" banner on the strength of an answer it has not received (a
 * subscribe going out and getting an `unsupported` frame back still wins
 * immediately regardless, since that is a definitive per-topic answer, not
 * a guess). */
export function deriveLiveState(
	sourceHasLiveFeed: boolean | undefined,
	unsupported: boolean,
	marketStatus?: MarketStatus
): LiveState {
	if (marketStatus?.status === 'closed') return marketStatus;
	if (unsupported) return { status: 'no-feed' };
	if (sourceHasLiveFeed === false) return { status: 'no-feed' };
	return { status: 'live' };
}

/** Whether a countdown belongs on screen for `state` — `live` only (the
 * a countdown against a feed that is not running is a lie). Pulled out as
 * its own function, rather than inlining
 * `state.status === 'live'` at each call site, so the "only live" rule is
 * one place to prove instead of several to keep in sync. */
export function showCountdown(state: LiveState): boolean {
	return state.status === 'live';
}

/** The candlestick series' `priceLineColor` for `state` — `''` (the
 * library's own default, which follows the last bar's colour) for `live`;
 * the fixed neutral token for anything else, set explicitly so a
 * non-streaming pane never shows a stale green/red line left over from
 * whenever it last had one. */
export function priceLineColorFor(state: LiveState): string {
	return state.status === 'live' ? '' : NEUTRAL_PRICE_LINE_COLOR;
}

/** The full sentence for `state`, or `null` in `live` state — used as the
 * pane header's live-state chip's hover title (native `title=`, not a
 * floating overlay: a persistent fact about the pane belongs in the
 * header's own flow, not hovering over the chips that already live there).
 * Exhaustively matched (the same discipline `Resource` gets server-side):
 * adding a fourth `LiveState` variant fails this function to compile until
 * it says what the reason text is for it. */
export function overlayMessage(state: LiveState): string | null {
	switch (state.status) {
		case 'live':
			return null;
		case 'no-feed':
			return 'This venue has no live feed. Showing stored history.';
		case 'closed':
			return state.opensAt == null
				? 'Market closed.'
				: `Market closed. Reopens ${new Date(state.opensAt / 1_000_000).toLocaleString()}.`;
		default: {
			const exhaustive: never = state;
			return exhaustive;
		}
	}
}

/** The pane header's live-state chip's own short label, or `null` in `live`
 * state (no chip at all — the common case must not clutter the header with
 * a permanent "LIVE" badge nobody asked for). `overlayMessage` above is the
 * same chip's hover title, spelling out the reason in full. */
export function liveChipLabel(state: LiveState): string | null {
	switch (state.status) {
		case 'live':
			return null;
		case 'no-feed':
			return 'NO LIVE FEED';
		case 'closed':
			return 'MARKET CLOSED';
		default: {
			const exhaustive: never = state;
			return exhaustive;
		}
	}
}
