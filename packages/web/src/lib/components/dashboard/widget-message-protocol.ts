// The wire shape of the small "Widget Host API" subset this build routes
// (`ready`, `config.get`, `config.patch`) — pulled out of
// `plugin-widget-frame.svelte` into its own pure module so the "is this
// message even shaped right" check can be unit-tested directly, without
// mounting a Svelte component, spinning up a real iframe, or dispatching a
// real `MessageEvent` (this project's own Svelte test harness renders
// server-side and never runs `onMount` or touches `window` at all — see
// `widget-frame.test.ts`'s own doc comment).
//
// This module answers exactly one question: does `data` look like a
// message a widget's own bundle is allowed to send? It says nothing about
// *who* sent it — `event.source`/`event.origin` are checked by the caller
// before this function is ever called, because neither question can be
// answered from the message body alone.

/** Every method this build's host actually routes. A widget's own SDK
 * declaring a method name outside this set gets nothing back — the same
 * "named but not yet available" discipline the server applies to an
 * extension point it does not recognize, applied here to methods within
 * the one extension point (`dashboard.widget`) this build does route. */
const KNOWN_METHODS = new Set(['ready', 'config.get', 'config.patch']);

export type WidgetHostMethod = 'ready' | 'config.get' | 'config.patch';

export interface WidgetToHostMessage {
	channel: 'senken.widget';
	v: 1;
	id: string;
	method: WidgetHostMethod;
	params?: { patch?: unknown };
}

/** `true` iff `data` is shaped exactly like a [`WidgetToHostMessage`] —
 * every field present, the right primitive type, `method` one of the
 * methods this build actually routes, and `id` a non-empty, bounded
 * string (long enough for any real correlation id, short enough that a
 * hostile bundle cannot use it to smuggle an arbitrarily large string
 * through a field this host never expects to hold much data).
 *
 * Deliberately conservative: an extra, unrecognized field on `data` does
 * not fail this check (a future SDK version may add one), but every field
 * this check *does* look at must match exactly — there is no partial
 * credit and no coercion (a numeric `v` of `"1"` is rejected, not
 * accepted as `1`). */
export function isWidgetToHostMessage(data: unknown): data is WidgetToHostMessage {
	if (typeof data !== 'object' || data === null) return false;
	const candidate = data as Record<string, unknown>;
	if (candidate.channel !== 'senken.widget') return false;
	if (candidate.v !== 1) return false;
	if (typeof candidate.id !== 'string' || candidate.id.length === 0 || candidate.id.length > 128) {
		return false;
	}
	if (typeof candidate.method !== 'string' || !KNOWN_METHODS.has(candidate.method)) return false;
	return true;
}

/** `true` iff a `message` event's `source`/`origin` are consistent with
 * having come from `iframe`'s own sandboxed document.
 *
 * A sandboxed iframe (`sandbox="allow-scripts"`, no `allow-same-origin`)
 * always gets an **opaque** origin, which a receiving `MessageEvent`
 * reports as the literal string `"null"` — regardless of what URL actually
 * served the iframe's document. Combined with `event.source` needing to be
 * this exact iframe's own `contentWindow`, this is what makes a message
 * claiming to be from the widget actually verifiable to be from *this*
 * widget's own sandboxed document, and not from some other frame, another
 * widget's iframe, a popup, or a hostile page that happens to be able to
 * call `postMessage` on this window for some other reason. */
export function isFromSandboxedWidget(
	event: Pick<MessageEvent, 'source' | 'origin'>,
	iframe: HTMLIFrameElement | undefined
): boolean {
	if (!iframe) return false;
	if (event.source !== iframe.contentWindow) return false;
	return event.origin === 'null';
}
