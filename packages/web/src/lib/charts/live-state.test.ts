import { describe, expect, test } from 'bun:test';
import { NEUTRAL_PRICE_LINE_COLOR, deriveLiveState, overlayMessage, priceLineColorFor, showCountdown } from './live-state';

describe('deriveLiveState — the sources cache and the WS unsupported frame combined', () => {
	test('a source known to have a live pool, with no unsupported frame, is live', () => {
		expect(deriveLiveState(true, false)).toEqual({ status: 'live' });
	});

	test('a source known to have no live pool is no-feed even with no WS answer yet', () => {
		expect(deriveLiveState(false, false)).toEqual({ status: 'no-feed' });
	});

	test('not yet knowing the source (GET /api/sources still in flight) defaults to live, not no-feed', () => {
		expect(deriveLiveState(undefined, false)).toEqual({ status: 'live' });
	});

	// The frame is the authoritative answer — it must win even when the
	// sources cache says otherwise (a stale cache, or a subscribe that fails
	// for some other in-session reason).
	test('an unsupported WS frame overrides a sources answer that claims live', () => {
		expect(deriveLiveState(true, true)).toEqual({ status: 'no-feed' });
	});

	test('a supplied closed status wins over the absence of a live feed', () => {
		const closed = { status: 'closed' as const };
		expect(deriveLiveState(false, false, closed)).toEqual(closed);
	});

	// Mutation proof: removing the `marketStatus` argument makes this fall
	// back to `no-feed`, so the status source is what protects this copy.
	test('without the supplied status the same source is correctly no-feed', () => {
		expect(deriveLiveState(false, false)).toEqual({ status: 'no-feed' });
	});
});

describe('showCountdown — "a countdown against a feed that is not running is a lie"', () => {
	test('only the live state shows a countdown', () => {
		expect(showCountdown({ status: 'live' })).toBe(true);
		expect(showCountdown({ status: 'no-feed' })).toBe(false);
		expect(showCountdown({ status: 'closed', opensAt: 0 })).toBe(false);
	});
});

describe('priceLineColorFor — library default vs. the explicit neutral token', () => {
	test('live leaves the line at the library default, which follows the candle colour', () => {
		expect(priceLineColorFor({ status: 'live' })).toBe('');
	});

	test('no-feed and closed both force the same explicit neutral colour', () => {
		expect(priceLineColorFor({ status: 'no-feed' })).toBe(NEUTRAL_PRICE_LINE_COLOR);
		expect(priceLineColorFor({ status: 'closed', opensAt: 0 })).toBe(NEUTRAL_PRICE_LINE_COLOR);
	});
});

describe('overlayMessage — never shown together with a countdown', () => {
	test('live has no overlay at all', () => {
		expect(overlayMessage({ status: 'live' })).toBeNull();
	});

	// Asserting the *claim* rather than its exact wording: display copy is
	// allowed to be reworded, but it must always name the missing feed and
	// must never imply a trading schedule this build does not have.
	test('no-feed names the reason, not a fabricated schedule', () => {
		const message = overlayMessage({ status: 'no-feed' });
		expect(message?.toLowerCase()).toContain('live feed');
		expect(message).not.toMatch(/open|close[ds]? at \d/i);
	});

	test('closed names the real next-open time it was given, once reachable', () => {
		const opensAt = Date.UTC(2026, 0, 2, 0, 0, 0) * 1_000_000; // nanoseconds
		const message = overlayMessage({ status: 'closed', opensAt });
		expect(message?.toLowerCase()).toMatch(/reopen/);
		// The instant it was handed, not a guess: the formatted date must
		// carry the year it was given.
		expect(message).toContain('2026');
	});

	// The property the whole union exists to guarantee: whenever there is a
	// countdown, there is no overlay, and vice versa — never both.
	test('showCountdown and overlayMessage never agree that both belong on screen', () => {
		const states: Parameters<typeof showCountdown>[0][] = [
			{ status: 'live' },
			{ status: 'no-feed' },
			{ status: 'closed', opensAt: 0 }
		];
		for (const state of states) {
			const hasCountdown = showCountdown(state);
			const hasOverlay = overlayMessage(state) !== null;
			expect(hasCountdown && hasOverlay).toBe(false);
		}
	});
});
