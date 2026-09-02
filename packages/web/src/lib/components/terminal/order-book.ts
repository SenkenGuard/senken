// Real order-book rows for `order-book.svelte`'s depth ladder — replaces
// `trade-demo.ts`'s former `genOrderBook`, which built a synthetic ladder
// around a real last-close price with a seeded PRNG. There is no synthetic
// fallback left: a venue without `book.supported` never reaches this module
// at all (`order-book.svelte` shows a disabled control with its reason
// instead), and a venue that does report the capability renders only what
// its own snapshot actually contains. Depth that merely *looks* real is
// worse than no depth at all: it is market data a reader would trade on.
//
// Kept pure (no `apiClient`, no `$state`) for the same reason
// `$lib/api/ws-frames.ts` and `$lib/charts/indicator-display.ts` are: it
// stays unit-testable with a plain `bun test` against the wire shape
// directly, without dragging in the WS client singleton.
import {
	bookFrameFor,
	isFailedFor,
	isUnsupportedFor,
	type WsBookLevel,
	type WsBookPayload
} from '$lib/api/ws-frames';
// Type-only, and it has to stay that way: `ws-events.svelte.ts` is a rune
// module, which `bun test` cannot import at all. `ws-frames.ts` depends on
// this shape the same way, for the same reason.
import type { WsEvent } from '$lib/api/ws-events.svelte';

export interface BookRow {
	price: string;
	size: string;
	side: 'ask' | 'bid';
	depthPct: number;
}

/** Below this, a real but tiny level would paint an all-but-invisible fill
 * — keeps every row's bar legible without falsely inflating its size. */
const MIN_DEPTH_PCT = 4;

/** `raw` at `scale` digits, formatted the same way this panel always has:
 * thousands-grouped past 1000, two decimals down to 10, four below that. */
function fmtScaled(raw: number, scale: number): string {
	const v = raw / 10 ** scale;
	if (v >= 1000) return v.toLocaleString('en-US', { maximumFractionDigits: 2, minimumFractionDigits: 2 });
	if (v >= 10) return v.toFixed(2);
	return v.toFixed(4);
}

/** Converts one `WsBookPayload`'s `bids`/`asks` (best price first on each
 * side, as `crates/api/src/ws.rs`'s own doc states — see `bookFrameFor`) into
 * the ladder's row order: asks reversed so the farthest ask paints at the
 * top and the best ask sits just above the spread, then bids as they already
 * arrive, so the best bid sits just below it — the same visual order the
 * panel has always used, now built from a real snapshot instead of a
 * generated one. `depthPct` is each level's size relative to the largest
 * size in the whole snapshot (both sides), not a random fill. */
export function bookRowsFromSnapshot(bids: WsBookLevel[], asks: WsBookLevel[], priceScale: number, qtyScale: number): BookRow[] {
	const maxSize = Math.max(0, ...bids.map((l) => l.size), ...asks.map((l) => l.size));
	const depthPct = (size: number): number => (maxSize <= 0 ? 0 : Math.max(MIN_DEPTH_PCT, Math.round((size / maxSize) * 100)));
	const toRow = (level: WsBookLevel, side: BookRow['side']): BookRow => ({
		price: fmtScaled(level.price, priceScale),
		size: fmtScaled(level.size, qtyScale),
		side,
		depthPct: depthPct(level.size)
	});
	return [...[...asks].reverse().map((l) => toRow(l, 'ask')), ...bids.map((l) => toRow(l, 'bid'))];
}

/** The venue's own timestamp for a snapshot, as the panel prints it —
 * `14:32:07`, in the reader's local zone, or `null` for a snapshot that has
 * not arrived or carries no usable time.
 *
 * The panel shows this instead of a "LIVE" badge, and that is deliberate.
 * Depth refreshes on its own now, so the only question a reader actually
 * has is whether what they are looking at is current — and a time that
 * stops advancing answers it truthfully in every case, including the ones a
 * badge would get wrong: a dropped websocket, a server that stopped
 * publishing, a tab that was backgrounded. A label claiming "LIVE" is a
 * promise about the next second; a timestamp is a fact about this one.
 *
 * Zero-padded and built from the parts rather than left to
 * `toLocaleTimeString`, so the width never changes under the reader (a
 * locale that drops the leading zero shifts the whole row at 09:59:59) and
 * so it cannot come back in a 12-hour form in a monospace ladder. */
export function formatBookTime(ts: number | null | undefined): string | null {
	if (typeof ts !== 'number' || !Number.isFinite(ts) || ts <= 0) return null;
	const at = new Date(ts);
	if (Number.isNaN(at.getTime())) return null;
	const pad = (n: number) => String(n).padStart(2, '0');
	return `${pad(at.getHours())}:${pad(at.getMinutes())}:${pad(at.getSeconds())}`;
}

/** Everything the panel knows about one `book:` topic, folded from the WS
 * frames that have arrived for it. */
export interface BookTopicState {
	/** The newest whole snapshot, or `null` before the first one. */
	snapshot: WsBookPayload | null;
	/** The server answered that this venue serves no depth at all. */
	unsupported: boolean;
	/** The server asked and could not get depth. */
	failed: boolean;
}

/** A topic nothing has been heard about yet — what a fresh subscribe, or a
 * change of instrument, resets to. */
export const NO_BOOK_YET: BookTopicState = { snapshot: null, unsupported: false, failed: false };

/** Folds one WS event into what the panel knows about `topic`.
 *
 * Pure, and separate from the component, so the one rule that is easy to get
 * wrong here can actually be tested: **a snapshot arriving clears a previous
 * failure.** The server's own poll loop keeps trying and recovers on its
 * own, so a failure flag that only ever gets set would leave a panel showing
 * "could not load depth" over a book that is arriving again every second —
 * a derived state disagreeing with the state it is derived from, which is
 * exactly the shape of bug this codebase keeps paying for.
 *
 * `unsupported` is not cleared the same way, and that asymmetry is
 * deliberate: "this venue serves no depth" is a fact about the venue that no
 * later frame contradicts, while "this attempt failed" is a fact about one
 * attempt that the next one does. */
export function foldBookEvent(
	previous: BookTopicState,
	event: WsEvent | null,
	topic: string
): BookTopicState {
	if (isUnsupportedFor(event, topic)) return { ...previous, unsupported: true };
	if (isFailedFor(event, topic)) return { ...previous, failed: true };
	const frame = bookFrameFor(event, topic);
	if (!frame) return previous;
	return { ...previous, snapshot: frame, failed: false };
}
