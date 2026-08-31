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
