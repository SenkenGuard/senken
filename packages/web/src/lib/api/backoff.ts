// Shared backoff-with-jitter helper ("reconnects with backoff and jitter"), used by both the REST heartbeat below (`client.ts`)
// and the WebSocket reconnect loop (`websocket.ts`) so the two don't grow
// two slightly different retry curves.
//
// Full jitter (a random delay in `[0, cap)`, not `cap` plus or minus a
// percentage) is the strategy AWS's "Exponential Backoff And Jitter"
// architecture post recommends over "equal" or "decorrelated" jitter for
// avoiding synchronized retry storms across many independent clients —
// relevant here because every open tab/panel reconnecting to the same
// restarted server is exactly that scenario.

export interface BackoffOptions {
	/** Delay before the first retry, in ms. */
	baseMs?: number;
	/** Never wait longer than this, in ms. */
	maxMs?: number;
	/** Multiplier applied per attempt. */
	factor?: number;
}

const DEFAULTS: Required<BackoffOptions> = { baseMs: 500, maxMs: 30_000, factor: 2 };

/** `attempt` is 0-based: the first retry after an initial failure is
 * `backoffDelay(0)`. */
export function backoffDelay(attempt: number, options: BackoffOptions = {}): number {
	const { baseMs, maxMs, factor } = { ...DEFAULTS, ...options };
	const cap = Math.min(maxMs, baseMs * factor ** attempt);
	return Math.random() * cap;
}
