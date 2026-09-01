import { describe, expect, test } from 'bun:test';
import {
	HISTORY_PAGE_BARS,
	HISTORY_PREFETCH_THRESHOLD_BARS,
	MAX_HISTORY_BARS,
	applyHistoryJump,
	historyLoadPriority,
	indicatorRange,
	historyWindowAround,
	previousHistoryPage,
	resolveJumpWindow,
	historyIsFull,
	shouldPrefetchHistory
} from './chart-window';

describe('chart history window', () => {
	test('left-edge paging requests the contiguous predecessor page', () => {
		const current = { from: 300, to: 600 };
		expect(previousHistoryPage(current)).toEqual({ from: 0, to: 300 });
		expect(shouldPrefetchHistory(80, HISTORY_PAGE_BARS)).toBe(true);
		expect(shouldPrefetchHistory(81, HISTORY_PAGE_BARS)).toBe(false);
		expect(historyLoadPriority()).toBe('prefetch');
	});

	test('a date jump replaces the old window instead of making two islands', () => {
		const jumped = historyWindowAround(10_000, 10);
		expect(jumped).toEqual({ from: 8_500, to: 11_500 });
		expect(jumped.to - jumped.from).toBe(HISTORY_PAGE_BARS * 10);
	});

	test('a full pane refuses another page instead of discarding the one it fetched', () => {
		expect(historyIsFull(MAX_HISTORY_BARS - 1)).toBe(false);
		expect(historyIsFull(MAX_HISTORY_BARS)).toBe(true);
	});

	// The cap exists to bound the heap, not to decide how far back a chart can
	// be read. At 20,000 it was the second thing: thirteen days of one-minute
	// bars, after which every drag left fetched a page and threw it away.
	test('the cap is deep enough to be a memory backstop rather than a viewing limit', () => {
		const minutesPerDay = 60 * 24;
		expect(MAX_HISTORY_BARS / minutesPerDay).toBeGreaterThan(30);
		const hoursPerYear = 24 * 365;
		expect(MAX_HISTORY_BARS / hoursPerYear).toBeGreaterThan(5);
	});

	test('a jump past the newest bar clamps to the trailing "now" window, not empty future space', () => {
		const now = 1_000_000;
		const barWidth = 10;
		const width = barWidth * HISTORY_PAGE_BARS;
		expect(resolveJumpWindow(now + 5_000, barWidth, now, null)).toEqual({ from: now - width, to: now });
		// Landing exactly on "now" is not "the future" — still a trailing window ending there.
		expect(resolveJumpWindow(now, barWidth, now, null)).toEqual({ from: now - width, to: now });
	});

	test('a jump before the known venue edge clamps to that edge instead of requesting an unfillable half', () => {
		const now = 1_000_000_000;
		const barWidth = 10;
		const width = barWidth * HISTORY_PAGE_BARS;
		const earliestAvailable = 500_000;
		// Centring on this target would ask for `from` well before the known edge.
		const target = earliestAvailable + 10;
		expect(resolveJumpWindow(target, barWidth, now, earliestAvailable)).toEqual({
			from: earliestAvailable,
			to: earliestAvailable + width
		});
	});

	test('an unknown edge does not clamp — the resulting fetch reports the edge itself', () => {
		const now = 1_000_000_000;
		const barWidth = 10;
		expect(resolveJumpWindow(1_000, barWidth, now, null)).toEqual(historyWindowAround(1_000, barWidth));
	});

	test('a jump landing cleanly away from both edges is exactly the centred window', () => {
		const now = 1_000_000_000;
		const barWidth = 10;
		const target = 500_000;
		expect(resolveJumpWindow(target, barWidth, now, 1)).toEqual(historyWindowAround(target, barWidth));
	});

	test('a jump replaces the loaded window instead of folding into it', () => {
		const current = [{ ts_open: 1 }, { ts_open: 2 }, { ts_open: 3 }];
		const resolved = [{ ts_open: 900 }, { ts_open: 901 }];
		const result = applyHistoryJump(current, resolved);
		expect(result).toEqual(resolved);
		// Proves replacement rather than concatenation: appending would have
		// left `current`'s entries in the result and grown its length.
		expect(result.length).toBe(resolved.length);
		expect(result.some((bar) => bar.ts_open < 900)).toBe(false);
	});
});

describe('prefetch hysteresis prevents thrashing on a small back-and-forth drag', () => {
	/** Mirrors the part of `requestOlderHistory` (`chart-pane.svelte`) that
	 * decides whether to fetch: track the viewport's own logical left edge
	 * as the reader nudges it, and fetch a page whenever
	 * `shouldPrefetchHistory` says to — exactly what the real effect calls.
	 * `pageBars` is parameterised only so the mutation test below can shrink
	 * it down to the threshold and show what breaks; the real pane always
	 * pages at `HISTORY_PAGE_BARS`. */
	function fetchesForDrag(moves: number[], pageBars: number): number {
		let loadedBars = pageBars;
		let position = loadedBars; // the viewport starts away from the loaded left edge
		let fetches = 0;
		for (const delta of moves) {
			position = Math.max(0, position + delta);
			if (!shouldPrefetchHistory(position, loadedBars)) continue;
			fetches += 1;
			loadedBars += pageBars;
			position += pageBars; // prepending shifts every already-visible index right
		}
		return fetches;
	}

	test('a small back-and-forth drag near the edge triggers far fewer fetches than moves', () => {
		// Net drift toward the edge, but every other move reverses briefly —
		// exactly the "small back-and-forth drag" B11a describes.
		const moves = Array.from({ length: 400 }, (_, i) => (i % 2 === 0 ? -30 : 22));
		const fetches = fetchesForDrag(moves, HISTORY_PAGE_BARS);
		expect(fetches).toBeGreaterThan(0);
		expect(fetches).toBeLessThan(moves.length / 4);
	});

	test('mutation: a page no bigger than the prefetch threshold reproduces the thrash this gap prevents', () => {
		const moves = Array.from({ length: 400 }, (_, i) => (i % 2 === 0 ? -30 : 22));
		const healthy = fetchesForDrag(moves, HISTORY_PAGE_BARS);
		// With no gap between the two thresholds, a fetch only ever pushes the
		// viewport exactly back to the trigger point — the next move re-fires
		// it, on nearly every remaining move.
		const thrashing = fetchesForDrag(moves, HISTORY_PREFETCH_THRESHOLD_BARS);
		expect(thrashing).toBeGreaterThan(healthy * 3);
	});
});

describe('indicatorRange', () => {
	const NS = 1_000_000_000;
	const bar = (ts: number) => ({ ts_open: ts });

	test('covers exactly the bars the pane holds', () => {
		const range = indicatorRange([bar(100 * NS), bar(160 * NS)], 60);
		expect(range).toEqual({ from: 100 * NS, to: 220 * NS });
	});

	test('widens when older history is prepended', () => {
		const loaded = [bar(100 * NS), bar(160 * NS)];
		const before = indicatorRange(loaded, 60);
		const after = indicatorRange([bar(40 * NS), ...loaded], 60);
		expect(after?.from).toBe(40 * NS);
		expect(after?.from).toBeLessThan(before?.from ?? 0);
		expect(after?.to).toBe(before?.to);
	});

	test('follows a jump to an older date instead of spanning to now', () => {
		const jumped = [bar(10 * NS), bar(70 * NS)];
		expect(indicatorRange(jumped, 60)).toEqual({ from: 10 * NS, to: 130 * NS });
	});

	test('is null while the pane has no bars', () => {
		expect(indicatorRange([], 60)).toBeNull();
	});
});
