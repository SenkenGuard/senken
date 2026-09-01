// Pure lookups over `GET /api/sources`' result — pulled out of
// `sources.svelte.ts` the same way `workspace-error.ts` pulls
// `layoutMutationErrorMessage` out of `workspace-store.svelte.ts`: that
// module's `$state` field needs the Svelte compiler to even load, so the
// actual decision logic lives here where a plain `bun test` can exercise it.
import type { SourceCapabilityDto } from './types';

/** `"okx-spot:BTCUSDT"` -> `"okx-spot"` — the left half of an
 * `InstrumentId`'s wire form, same split `chart-config.ts`'s
 * `parseInstrumentId` already uses. */
export function sourceIdOf(instrument: string): string {
	const [source] = instrument.split(':');
	return source ?? instrument;
}

/** Whether `instrument`'s source has a live feed pool in this build. An id
 * absent from `sources` (not yet loaded, or not a registered source at all)
 * is treated as "no feed" — the safe default: a countdown or a green/red
 * price line must never be shown on the strength of an *absent* answer. */
export function hasLiveFeed(sources: ReadonlyMap<string, SourceCapabilityDto>, instrument: string): boolean {
	return sources.get(sourceIdOf(instrument))?.live ?? false;
}

/** Whether `instrument`'s source explicitly reports best-bid-and-offer
 * updates. A live last-trade feed is not enough: quote lines must only be
 * offered when this separate capability is present. */
export function hasQuoteFeed(sources: ReadonlyMap<string, SourceCapabilityDto>, instrument: string): boolean {
	return sources.get(sourceIdOf(instrument))?.quotes ?? false;
}

/** Whether `instrument`'s source has a bar source registered at all — a
 * source can chart without streaming (`bars: true, live: false`), but never
 * the reverse (`SourceCapabilityDto`'s own doc: "never true without
 * `bars`"). */
export function hasBarSource(sources: ReadonlyMap<string, SourceCapabilityDto>, instrument: string): boolean {
	return sources.get(sourceIdOf(instrument))?.bars ?? false;
}

/** Whether `instrument`'s source can serve the order-book panel a
 * fixed-depth snapshot — `book` is a nested object (`{ supported: boolean }`),
 * not a fourth flat flag, so this reads `.book?.supported` rather than
 * `.book` itself. Same absent-is-false default as every other capability
 * read here: the panel must never render as if it works on the strength of
 * an answer that has not arrived yet. */
export function hasBookFeed(sources: ReadonlyMap<string, SourceCapabilityDto>, instrument: string): boolean {
	return sources.get(sourceIdOf(instrument))?.book?.supported ?? false;
}
