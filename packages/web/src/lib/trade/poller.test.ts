import { describe, expect, test } from 'bun:test';
import { TradePoller, type PollerParams } from './poller';

/** A promise plus its own resolver, pulled apart. Not `let x = null` plus a
 * later reassignment inside the executor: TypeScript's control-flow
 * narrowing does not see across that closure boundary and keeps reading the
 * variable as `null` at every later call site, even though the executor has
 * already run synchronously by then. A definite-assignment declaration with
 * no `null` in its flow sidesteps it. */
function deferred<T = void>(): { promise: Promise<T>; resolve: (value: T) => void } {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((res) => {
		resolve = res;
	});
	return { promise, resolve };
}

/** A controllable stand-in for the browser primitives `TradePoller` takes:
 * `setInterval`/`clearInterval` never actually schedule anything — the
 * captured handler is invoked by hand to simulate time passing, so every
 * test below runs in zero real time and cannot flake on a slow machine. */
function harness(overrides: Partial<PollerParams<number>> = {}) {
	let nextHandle = 1;
	const intervalCalls: { handler: () => void; ms: number; handle: number }[] = [];
	const clearedHandles: number[] = [];
	let visibilityHandler: (() => void) | null = null;
	let unsubscribeCalls = 0;
	let hidden = false;
	let tickCalls = 0;

	const params: PollerParams<number> = {
		intervalMs: 5000,
		tick: async () => {
			tickCalls++;
		},
		isHidden: () => hidden,
		setInterval: (handler, ms) => {
			const handle = nextHandle++;
			intervalCalls.push({ handler, ms, handle });
			return handle;
		},
		clearInterval: (handle) => {
			clearedHandles.push(handle);
		},
		onVisibilityChange: (handler) => {
			visibilityHandler = handler;
			return () => {
				unsubscribeCalls++;
				visibilityHandler = null;
			};
		},
		...overrides
	};

	const poller = new TradePoller<number>(params);
	return {
		poller,
		params,
		fireInterval: () => intervalCalls[intervalCalls.length - 1]?.handler(),
		fireVisibilityChange: () => visibilityHandler?.(),
		intervalCalls: () => intervalCalls,
		clearedHandles: () => clearedHandles,
		unsubscribeCalls: () => unsubscribeCalls,
		setHidden: (value: boolean) => (hidden = value),
		tickCalls: () => tickCalls
	};
}

describe('TradePoller', () => {
	test('ticks on the interval', async () => {
		const h = harness();
		h.poller.start();
		expect(h.intervalCalls()).toHaveLength(1);
		expect(h.intervalCalls()[0]?.ms).toBe(5000);

		await h.fireInterval();
		expect(h.tickCalls()).toBe(1);
		await h.fireInterval();
		expect(h.tickCalls()).toBe(2);
	});

	test('does not tick while hidden', async () => {
		const h = harness();
		h.setHidden(true);
		h.poller.start();

		await h.fireInterval();
		expect(h.tickCalls()).toBe(0);
	});

	test('refreshes once, immediately, when visibility returns', async () => {
		const h = harness();
		h.setHidden(true);
		h.poller.start();

		// The tab itself becoming hidden also fires `visibilitychange` —
		// that must not trigger a tick.
		await h.fireVisibilityChange();
		expect(h.tickCalls()).toBe(0);

		h.setHidden(false);
		await h.fireVisibilityChange();
		expect(h.tickCalls()).toBe(1);

		// And the interval keeps working afterwards.
		await h.fireInterval();
		expect(h.tickCalls()).toBe(2);
	});

	test('skips a tick while one is in flight, and resumes once it settles', async () => {
		let tickCalls = 0;
		const tickDeferred = deferred();
		const h = harness({
			tick: () => {
				tickCalls++;
				return tickDeferred.promise;
			}
		});
		h.poller.start();

		const firstTick = h.fireInterval();
		expect(tickCalls).toBe(1); // in flight

		// A second interval firing while the first is still pending must be
		// skipped outright, not queued for when the first settles.
		await h.fireInterval();
		expect(tickCalls).toBe(1);

		tickDeferred.resolve();
		await firstTick;

		// Now that the in-flight tick has settled, the timer ticks again.
		await h.fireInterval();
		expect(tickCalls).toBe(2);
	});

	test('reference counting: two starts share one timer; it stops only once both release', async () => {
		const h = harness();
		const stopA = h.poller.start();
		const stopB = h.poller.start();
		expect(h.intervalCalls()).toHaveLength(1); // one shared timer, not two

		stopA();
		expect(h.clearedHandles()).toHaveLength(0); // still running — B hasn't released
		await h.fireInterval();
		expect(h.tickCalls()).toBe(1); // still ticking

		stopB();
		expect(h.clearedHandles()).toHaveLength(1);
		expect(h.unsubscribeCalls()).toBe(1);

		// A stop function is idempotent: releasing an already-released
		// caller must not double-decrement and clear something already
		// clear, or throw.
		stopA();
		stopB();
		expect(h.clearedHandles()).toHaveLength(1);
	});

	test('a rejected tick does not stop the timer', async () => {
		const h = harness({
			tick: async () => {
				throw new Error('adapter unreachable');
			}
		});
		h.poller.start();

		await h.fireInterval();
		expect(h.clearedHandles()).toHaveLength(0);

		// The timer is still alive and still ticking on the next firing.
		await h.fireInterval();
		expect(h.clearedHandles()).toHaveLength(0);
	});

	test('a fresh start after every caller released rebuilds the timer', async () => {
		const h = harness();
		const stop = h.poller.start();
		stop();
		expect(h.clearedHandles()).toHaveLength(1);

		h.poller.start();
		expect(h.intervalCalls()).toHaveLength(2); // a second, fresh timer
	});
});
