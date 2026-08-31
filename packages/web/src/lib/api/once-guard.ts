// The primitive behind B16 point 2's "exactly once, not once per in-flight
// request." A dead session 401s on every request that was in flight when it
// died, and on every one fired afterwards until something reacts — without
// this, each of those independently clears an already-cleared credential
// and re-fires the "route to login" side effect, which is the concurrent-
// 401 redirect loop B16 names explicitly.
//
// Deliberately its own tiny, rune-free class: `ApiClient` (`client.ts`) is
// otherwise coupled to `$state`-bearing modules that only load under
// Svelte's compiler, so this is what lets the dedup behaviour itself be
// unit-tested with a plain `bun test`.
export class OnceGuard {
	private armed = true;

	/** Runs `effect` the first time this is called since construction or
	 * the last `reset()`; every call after that is a silent no-op. */
	fire(effect: () => void): void {
		if (!this.armed) return;
		this.armed = false;
		effect();
	}

	/** Re-arms the guard so the next `fire` runs its effect again — call
	 * this once a fresh session has been established (a login), so a
	 * *later*, genuinely new expiry isn't swallowed by the previous one's
	 * guard. */
	reset(): void {
		this.armed = true;
	}
}
