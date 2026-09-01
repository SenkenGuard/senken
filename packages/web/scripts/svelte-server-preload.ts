// Test-only `.svelte` loader for `bun test`. Vite's own Svelte plugin never
// runs here — `bun test` bypasses Vite entirely — so a test that imports a
// `.svelte` file needs its own compile step or the import fails outright.
//
// This compiles straight to Svelte's server output (`generate: 'server'`),
// the same compiler `svelte/server`'s own `render()` consumes, so a test can
// assert on the component's *actual* rendered markup — real
// `data-*` attributes and real text content — rather than on a description
// of what the component is supposed to do. That is the only way to satisfy
// "verified in the DOM, not by eye" for a component this project has no
// browser/jsdom harness to mount interactively.
//
// Deliberately narrow: this exists for presentational, dependency-free
// components (props in, markup out — no lifecycle, no browser API calls).
// A component that touches `ResizeObserver`/canvas/websockets during
// `onMount` — `chart-pane.svelte` itself — cannot be rendered this way and
// is not what this preload is for.
import { plugin } from 'bun';
import { compile } from 'svelte/compiler';

plugin({
	name: 'svelte-server-compile',
	setup(build) {
		build.onLoad({ filter: /\.svelte$/ }, async (args) => {
			const source = await Bun.file(args.path).text();
			const compiled = compile(source, { generate: 'server', filename: args.path });
			return { contents: compiled.js.code, loader: 'js' };
		});
	}
});
