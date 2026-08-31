import { describe, expect, test } from 'bun:test';
import { advanceDeadline, formatCountdown } from './countdown';

describe('advanceDeadline — rolling a bar boundary forward without reinventing it', () => {
	test('a deadline still in the future is untouched', () => {
		expect(advanceDeadline(10_000, 1_000, 5_000)).toBe(10_000);
	});

	test('a deadline exactly at "now" rolls forward by exactly one step', () => {
		expect(advanceDeadline(10_000, 1_000, 10_000)).toBe(11_000);
	});

	// The case this exists for: the countdown reaches zero and nothing has
	// re-fetched `next_bar_open_at` yet — it must roll forward by the bar's
	// own duration, landing back in the future, not sit at (or below) zero.
	test('a deadline just past "now" rolls forward until it is in the future again', () => {
		expect(advanceDeadline(10_000, 1_000, 10_500)).toBe(11_000);
	});

	// A backgrounded tab or a slow tick can miss several bar closes at once —
	// this must catch all the way up, not just one step.
	test('a deadline missed by several whole steps advances by all of them at once', () => {
		expect(advanceDeadline(10_000, 1_000, 13_400)).toBe(14_000);
	});

	test('a non-positive step is refused rather than looping forever', () => {
		expect(advanceDeadline(10_000, 0, 50_000)).toBe(10_000);
		expect(advanceDeadline(10_000, -1, 50_000)).toBe(10_000);
	});
});

describe('formatCountdown — the price-axis label text', () => {
	test('under a minute', () => {
		expect(formatCountdown(5_000)).toBe('0:05');
	});

	test('minutes and seconds, no leading hour', () => {
		expect(formatCountdown(65_000)).toBe('1:05');
	});

	test('at exactly one hour, the hour segment appears', () => {
		expect(formatCountdown(3_600_000)).toBe('1:00:00');
	});

	test('hours, minutes and seconds all padded', () => {
		expect(formatCountdown(3_661_000)).toBe('1:01:01');
	});

	test('a negative remainder (deadline momentarily behind now) clamps to zero rather than going negative', () => {
		expect(formatCountdown(-500)).toBe('0:00');
	});
});
