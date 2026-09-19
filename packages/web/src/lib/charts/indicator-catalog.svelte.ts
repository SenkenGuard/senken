// One shared poll for `GET /api/indicators`, instead of the one
// `setInterval` per chart pane this replaces. With several panes open (a
// multi-pane workspace, or several chart tabs) the old shape polled the same
// endpoint once per pane, every 15 seconds, for identical data — and every
// poll reassigned each pane's own `$state<Set<string>>` to a brand-new `Set`
// even when its contents had not changed, which was enough to re-trigger a
// pane's overlay/sub-pane reconciliation effects on a timer, independent of
// whether the loaded range or the layer set had actually moved
// (`chart-pane.svelte`'s own `loadKey`/`lastLoadedOverlays" guards exist
// because of exactly that).
//
// A pane now subscribes here instead: the poll starts with the first
// subscriber and stops with the last, so two panes — or twenty — cost one
// interval and one fetch per tick, not one each.
import { fetchIndicatorCatalog } from './indicator-catalog';

const DEFAULT_POLL_MS = 15_000;

let catalog = $state<Set<string>>(new Set());
let subscriberCount = 0;
let pollTimer: ReturnType<typeof setInterval> | null = null;

function poll(): void {
	fetchIndicatorCatalog()
		.then((names) => {
			catalog = names;
		})
		.catch(() => {
			// Transient — keep whatever was already known rather than treating a
			// network blip as "every dynamic indicator was just disabled".
		});
}

/** The current indicator catalogue: the ten built-ins, always present, plus
 * every dynamic indicator loaded from an uploaded `.wasm` component that is
 * not currently disabled.
 *
 * Read this directly inside the effect or derived value that needs it —
 * never copy the returned `Set` into a component's own local `$state` and
 * read that back later. A copy taken once would freeze at whatever the
 * catalogue held at that moment, the exact "derive from what you opened
 * with" bug this module exists to keep out of every caller. */
export function indicatorCatalog(): Set<string> {
	return catalog;
}

/** Keeps the catalogue polling for as long as at least one caller holds a
 * subscription; call the returned function to release it. The poll itself
 * is shared — a second, tenth, twentieth subscriber joins the interval
 * already running rather than starting one of its own — and it stops the
 * moment the last subscriber releases, rather than leaking a timer for the
 * life of the page.
 *
 * `intervalMs` only takes effect when it starts the poll (the first
 * subscriber while none was already running); a later subscriber joins
 * whatever interval is already active. Tests shorten it to observe a second
 * poll tick without a real 15-second wait. */
export function subscribeIndicatorCatalog(intervalMs = DEFAULT_POLL_MS): () => void {
	subscriberCount += 1;
	if (subscriberCount === 1) {
		poll();
		pollTimer = setInterval(poll, intervalMs);
	}
	let released = false;
	return () => {
		if (released) return;
		released = true;
		subscriberCount -= 1;
		if (subscriberCount === 0 && pollTimer !== null) {
			clearInterval(pollTimer);
			pollTimer = null;
		}
	};
}

/** Forces an immediate re-poll, independent of the interval — what a
 * successful indicator save calls so a newly compiled (or just-disabled)
 * indicator appears in every open pane without waiting out the interval. */
export function refreshIndicatorCatalog(): void {
	poll();
}
