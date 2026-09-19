// The catalogue poll this module wraps used to be one `setInterval` per
// chart pane, each firing `GET /api/indicators` on its own timer — with N
// panes open, N redundant calls for identical data every interval tick, and
// every one of them reassigned that pane's own `$state<Set<string>>` to a
// brand-new `Set` even when nothing about the catalogue had changed. This
// proves the shared replacement: two subscribers cost one poll, which starts
// with the first subscriber and stops only once the last one releases.
//
// This lives beside `indicator-catalog.svelte.ts` rather than as a plain
// `bun:test` file — a pure Bun test can't exercise this, because
// `fetchIndicatorCatalog` (`./indicator-catalog.ts`) pulls in
// `$lib/api/client`, whose own import graph eagerly constructs a
// `$state`-bearing singleton (`$lib/state/settings.svelte.ts`) the moment
// the module loads — `$state` is a compiler rune, not a runtime function,
// and `bun test` never runs the Svelte compiler over it. Vitest's browser
// mode does (the same real-browser, real-compiler harness the chart-pane
// tests already use), so the property is proved there instead.
import { test, expect, vi } from 'vitest';
import { indicatorCatalog, subscribeIndicatorCatalog } from './indicator-catalog.svelte';

function catalogResponse(): Response {
	return new Response(JSON.stringify([{ name: 'Sma' }]), {
		status: 200,
		headers: { 'Content-Type': 'application/json' }
	});
}

test('two panes share one catalog poll', async () => {
	let calls = 0;
	vi.stubGlobal(
		'fetch',
		vi.fn(async () => {
			calls += 1;
			return catalogResponse();
		})
	);

	// A tiny interval — never the real 15s default — so several ticks land
	// well inside this test's own timeout.
	const unsubscribeA = subscribeIndicatorCatalog(20);
	const unsubscribeB = subscribeIndicatorCatalog(20);

	// The first subscriber's own immediate poll, shared by the second: two
	// subscribers joining at once must not mean two immediate polls.
	await new Promise((resolve) => setTimeout(resolve, 10));
	expect(calls).toBe(1);
	expect(indicatorCatalog().has('Sma')).toBe(true);

	// Several ticks pass on the one shared interval.
	await new Promise((resolve) => setTimeout(resolve, 90));
	const afterBothSubscribed = calls;
	expect(afterBothSubscribed).toBeGreaterThan(1);

	// Releasing one of two subscribers must not stop the shared poll — the
	// other one still holds it open.
	unsubscribeA();
	await new Promise((resolve) => setTimeout(resolve, 60));
	expect(calls).toBeGreaterThan(afterBothSubscribed);

	// Releasing the last subscriber stops it for good.
	unsubscribeB();
	const afterAllReleased = calls;
	await new Promise((resolve) => setTimeout(resolve, 60));
	expect(calls).toBe(afterAllReleased);

	vi.unstubAllGlobals();
});
