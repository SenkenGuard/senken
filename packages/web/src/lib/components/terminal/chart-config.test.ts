import { describe, expect, test } from 'bun:test';
import { isIntradaySpec } from './chart-config';

describe('isIntradaySpec — drives the time axis\' timeVisible/secondsVisible', () => {
	test('every sub-daily spec is intraday', () => {
		for (const spec of ['1m', '5m', '15m', '1h', '4h']) {
			expect(isIntradaySpec(spec)).toBe(true);
		}
	});

	test('1d and 1w are not intraday — a date is the informative label there', () => {
		expect(isIntradaySpec('1d')).toBe(false);
		expect(isIntradaySpec('1w')).toBe(false);
	});

	test('an unrecognised spec defaults to intraday, matching every other TF_DURATION_SECONDS fallback in this file', () => {
		expect(isIntradaySpec('30s')).toBe(true);
	});
});
