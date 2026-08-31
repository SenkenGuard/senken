import { describe, expect, test } from 'bun:test';
import { GenerationGuard } from './generation-guard';

describe('GenerationGuard — the race A9b describes', () => {
	test('a single run is current until something newer begins', () => {
		const guard = new GenerationGuard();
		const token = guard.begin();
		expect(guard.isCurrent(token)).toBe(true);
	});

	test('starting a newer run supersedes the older one immediately, before either resolves', () => {
		const guard = new GenerationGuard();
		const older = guard.begin(); // e.g. a layer shown with volume+sma
		const newer = guard.begin(); // e.g. the same layer hidden, moments later
		expect(guard.isCurrent(older)).toBe(false);
		expect(guard.isCurrent(newer)).toBe(true);
	});

	// This is the exact scenario A9b names: two visibility toggles inside
	// one animation frame start two overlapping async reconciliations, and
	// whichever happens to resolve *last* is not necessarily the newer one.
	// Without this guard, the older run's write still lands — overwriting
	// (or, per A9b, removing series out from under) whatever the newer run
	// already drew. Commenting out the `isCurrent` check at the call site
	// (in `chart-pane.svelte`'s indicator effect) reproduces exactly that:
	// this test's second assertion below then fails because the older run
	// is treated as current again. Restored after confirming that.
	test("the older run's write is refused even when it resolves after the newer one", () => {
		const guard = new GenerationGuard();
		const older = guard.begin();
		const newer = guard.begin();

		// Newer resolves first (e.g. served from cache).
		const newerApplied = guard.isCurrent(newer);
		// Older resolves second (e.g. a slower network round trip) — this is
		// the write that must be refused.
		const olderApplied = guard.isCurrent(older);

		expect(newerApplied).toBe(true);
		expect(olderApplied).toBe(false);
	});

	test('three overlapping runs: only the very last begin() is ever current', () => {
		const guard = new GenerationGuard();
		const a = guard.begin();
		const b = guard.begin();
		const c = guard.begin();
		expect([guard.isCurrent(a), guard.isCurrent(b), guard.isCurrent(c)]).toEqual([false, false, true]);
	});
});
