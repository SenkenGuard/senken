import { describe, expect, test } from 'bun:test';
import { OnceGuard } from './once-guard';

describe('OnceGuard — the concurrent-401 redirect-loop guard', () => {
	test('fires the effect on the first call', () => {
		const guard = new OnceGuard();
		let calls = 0;
		guard.fire(() => calls++);
		expect(calls).toBe(1);
	});

	test('does not fire again on subsequent calls', () => {
		const guard = new OnceGuard();
		let calls = 0;
		guard.fire(() => calls++);
		guard.fire(() => calls++);
		guard.fire(() => calls++);
		expect(calls).toBe(1);
	});

	test('simulates several concurrent 401s from in-flight requests: exactly one side effect', async () => {
		const guard = new OnceGuard();
		let sideEffects = 0;
		const clearCredentialAndRedirect = () => {
			sideEffects++;
		};

		// Five "requests" all discover the dead session at slightly different
		// times, the way real concurrent `fetch` rejections would.
		await Promise.all(
			[3, 1, 4, 1, 5].map(
				(ms) =>
					new Promise<void>((resolve) => {
						setTimeout(() => {
							guard.fire(clearCredentialAndRedirect);
							resolve();
						}, ms);
					})
			)
		);

		expect(sideEffects).toBe(1);
	});

	test('reset() re-arms the guard for a genuinely new session', () => {
		const guard = new OnceGuard();
		let calls = 0;
		guard.fire(() => calls++);
		guard.fire(() => calls++); // swallowed
		guard.reset(); // e.g. a fresh login
		guard.fire(() => calls++);
		expect(calls).toBe(2);
	});
});
