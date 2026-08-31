import { describe, expect, test } from 'bun:test';
import { backoffDelay } from './backoff';

describe('backoffDelay', () => {
	test('full jitter: delay is always within [0, cap)', () => {
		for (let attempt = 0; attempt < 8; attempt++) {
			for (let i = 0; i < 50; i++) {
				const delay = backoffDelay(attempt, { baseMs: 100, maxMs: 10_000, factor: 2 });
				expect(delay).toBeGreaterThanOrEqual(0);
				expect(delay).toBeLessThan(100 * 2 ** attempt);
			}
		}
	});

	test('caps growth at maxMs rather than growing unboundedly', () => {
		for (let i = 0; i < 50; i++) {
			const delay = backoffDelay(20, { baseMs: 500, maxMs: 5_000, factor: 2 });
			expect(delay).toBeLessThan(5_000);
		}
	});

	test('is not deterministic — two calls for the same attempt can differ (jitter)', () => {
		const samples = new Set(
			Array.from({ length: 20 }, () => backoffDelay(5, { baseMs: 1000, maxMs: 60_000 }))
		);
		// Extremely unlikely to collide 20 times in a row if jitter is real.
		expect(samples.size).toBeGreaterThan(1);
	});
});
