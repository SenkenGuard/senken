// Proves the full `theme.changed` round trip against a real sandboxed
// iframe, in a real browser — deliberately independent of this build's
// actual compiled-in `example-clock` widget. That widget's own HTML
// (`plugins/widgets/example-clock/web/index.html`) is baked into
// the Rust binary through `include_str!` at *compile* time, not read off
// disk the way `packages/web/build`'s own assets are (see
// `crates/api/src/assets.rs`'s own doc comment on that distinction) — so a
// change to that file only reaches a running `senken serve` after a
// `cargo build`, which this test file does not invoke.
//
// Getting a *second* real document to actually execute inside the
// component's own sandboxed iframe, in this test runner, took three failed
// attempts worth knowing about: neither a `data:` URL (any sandbox) nor a
// `blob:` URL (sandboxed or not) ever navigated at all here — this
// environment nests the test's own page inside a further iframe of its
// own, and `window.top.postMessage` from three frames deep never reached
// this file's listener either, even once a document did load via
// `srcdoc`. `srcdoc` plus `window.parent.postMessage` (one hop, not two)
// is what actually works, so the iframe the real component renders is
// re-pointed at a `srcdoc` document directly on the DOM node after
// mount — the component itself never sets `srcdoc`; this substitutes only
// *what the browser loads into it*, never anything about the component's
// own message-handling code, which runs exactly as it does in production.
import { test, expect, afterEach } from 'vitest';
import { page } from 'vitest/browser';
import { render } from 'vitest-browser-svelte';
import PluginWidgetFrame from './plugin-widget-frame.svelte';
import { setMode } from 'mode-watcher';

/** A widget document, parameerised by `nonce`: sends `ready` on load (the
 * same SDK call `example-clock`'s own bundle makes), then forwards every
 * `theme.changed` message it receives straight back to the host, tagged
 * with this same `nonce` — the one thing a real widget bundle would never
 * do, added purely so this test can observe from the outside what only the
 * sandboxed document itself can see. The `nonce` exists because this
 * component's own effect re-sends a theme on *every* `mode.current`
 * change, including `afterEach`'s own reset below — without it, a
 * still-in-flight, animation-frame-deferred message from one test's own
 * teardown can be the message the *next* test's listener happens to catch,
 * since both listen on the same top-level `window`. */
function relayWidgetHtml(nonce: string): string {
	return `<!doctype html><script>
		window.parent.postMessage({ channel: 'senken.widget', v: 1, id: 'r1', method: 'ready' }, '*');
		window.addEventListener('message', (event) => {
			const data = event.data;
			if (!data || data.channel !== 'senken.widget' || data.method !== 'theme.changed') return;
			window.parent.postMessage({ tag: 'senken.widget.test.themeReceived', nonce: '${nonce}', params: data.params }, '*');
		});
	<\/script>`;
}

interface ThemeReceived {
	mode: 'dark' | 'light';
	tokens: Record<string, string>;
}

function waitForThemeReceived(nonce: string): Promise<ThemeReceived> {
	return new Promise((resolve) => {
		function onMessage(event: MessageEvent) {
			if (event.data?.tag !== 'senken.widget.test.themeReceived') return;
			if (event.data?.nonce !== nonce) return;
			window.removeEventListener('message', onMessage);
			resolve(event.data.params as ThemeReceived);
		}
		window.addEventListener('message', onMessage);
	});
}

/** Two representative tokens set directly on `document.documentElement`'s
 * own inline style — the same place `getComputedStyle` in
 * `currentThemeTokens` reads from, and exactly how a real value lands
 * there whether it came from a stylesheet rule or (as here) an inline
 * property. This is deliberately not the app's real `layout.css`: an
 * isolated component mount never loads it (see
 * `command-palette.browser-test.ts`'s own note on why a computed-style
 * assertion in an isolated mount would otherwise pass for the wrong
 * reason), so this test supplies its own controlled values instead of
 * asserting against Tailwind output it never loaded. */
function setFakeThemeTokens(fg: string): void {
	document.documentElement.style.setProperty('--fg', fg);
	document.documentElement.style.setProperty('--font-mono', 'ui-monospace');
}

afterEach(() => {
	setMode('light');
	document.documentElement.style.removeProperty('--fg');
	document.documentElement.style.removeProperty('--font-mono');
});

test('the host sends theme.changed with real, non-empty tokens as soon as the widget says ready', async () => {
	setFakeThemeTokens('#112233');
	const nonce = 'ready-test';

	await render(PluginWidgetFrame, {
		entryUrl: 'about:blank',
		widgetTypeId: 'example/clock',
		config: '{}'
	});
	const received = waitForThemeReceived(nonce);
	const iframe = page.getByTitle('example/clock').element() as HTMLIFrameElement;
	iframe.srcdoc = relayWidgetHtml(nonce);

	const theme = await received;
	expect(theme.mode === 'dark' || theme.mode === 'light').toBe(true);
	// `--fg` and `--font-mono` stand in for the fixed token list
	// (`widget-message-protocol.ts`'s own `THEME_TOKEN_NAMES`) — a
	// representative color token this test set and the one non-color token
	// both arriving with their real values is enough to show
	// `currentThemeTokens` actually read `getComputedStyle`, not just the
	// right key names.
	expect(theme.tokens['--fg']).toBe('#112233');
	expect(theme.tokens['--font-mono']).toBe('ui-monospace');
});

test('toggling the theme mode sends a fresh theme.changed with different tokens, no reload', async () => {
	setFakeThemeTokens('#112233');
	const nonce = 'toggle-test';

	await render(PluginWidgetFrame, {
		entryUrl: 'about:blank',
		widgetTypeId: 'example/clock',
		config: '{}'
	});
	const firstReceived = waitForThemeReceived(nonce);
	const iframe = page.getByTitle('example/clock').element() as HTMLIFrameElement;
	iframe.srcdoc = relayWidgetHtml(nonce);
	const first = await firstReceived;

	const secondReceived = waitForThemeReceived(nonce);
	const otherMode = first.mode === 'dark' ? 'light' : 'dark';
	// A real theme change touches its tokens too, not just its mode name —
	// changing the fake `--fg` alongside `setMode` is what proves this is
	// a *fresh* read on the second send, not the first message's own
	// object handed back unchanged.
	setFakeThemeTokens('#eeddcc');
	// The same call the app's own theme toggle makes (`nav-rail.svelte`) —
	// this is what actually updates `mode-watcher`'s `derivedMode`, the
	// store the component's own effect watches.
	setMode(otherMode);

	const second = await secondReceived;
	expect(second.mode).toBe(otherMode);
	expect(second.tokens['--fg']).toBe('#eeddcc');
	expect(second.tokens['--fg']).not.toBe(first.tokens['--fg']);
});
