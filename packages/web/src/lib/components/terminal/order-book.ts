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
import type { WsBookLevel } from '$lib/api/ws-frames';

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
