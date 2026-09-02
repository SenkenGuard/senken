// A dynamic indicator's plugin being disabled must remove its plot without
// ever moving the reader's viewport — the same class of bug
// `viewport-writes.test.ts` guards against (a re-run effect that
// re-derives a *frozen* value instead of reading current state), applied to
// the one new code path `indicatorCatalog` adds: the catalogue filter that
// turns "not in `GET /api/indicators` right now" into a placeholder.
//
// The historical failure was an effect asking again for the *opening* bar
// window instead of the range the pane is holding right now
// (`loadedRange`'s own doc comment in `chart-pane.svelte`). This is a
// structural guard, in the same spirit as `viewport-writes.test.ts`: it
// proves the catalogue filter this slice adds always sits beside a fresh
// read of `loadedRange`, in both places a layer's indicator name is
// checked against the catalogue, and that no second, frozen copy of the
// range exists anywhere in the file for it to have used instead.

import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';

const PANE = 'src/lib/components/terminal/chart-pane.svelte';

function sourceLines(): string[] {
	return readFileSync(PANE, 'utf8').split('\n');
}

describe('the indicator catalogue filter never re-derives its own range', () => {
	test('both batch indicator effects read `loadedRange` fresh near the catalogue check', () => {
		const lines = sourceLines();
		const catalogChecks = lines
			.map((line, index) => ({ line, index }))
			.filter(({ line }) => line.includes('catalog.has('));

		// Not just "at least one" — the overlay effect and the native
		// sub-pane effect each have their own catalogue check, and a future
		// edit that drops one silently would not be caught by a looser
		// count.
		expect(catalogChecks.length).toBe(2);

		for (const { index } of catalogChecks) {
			// The nearest `loadedRange` reference within the same effect
			// body — a line-window stand-in for "same function scope", the
			// same technique `viewport-writes.test.ts`'s own `isHeld` uses
			// rather than parsing the component as an AST.
			const window = lines.slice(Math.max(0, index - 25), index + 5);
			expect(window.some((line) => line.includes('loadedRange'))).toBe(true);
		}
	});

	test('no second, frozen range variable exists for the catalogue filter to have used instead', () => {
		const text = sourceLines().join('\n');
		// The historical bug's own variable would have been named for the
		// window it opened with, not the state the pane now holds — this
		// class of name is what a reintroduction would look like.
		expect(text).not.toMatch(/\b(?:initial|opening)Range\b/);
	});
});
