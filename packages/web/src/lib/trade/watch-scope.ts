// Which accounts a refresh tick should actually fetch.
//
// A plain, framework-free counter for the same reason `poller.ts` is one:
// `state/trade.svelte.ts` cannot be imported under `bun test` (rune modules
// have no meaning outside the Svelte compiler), so the rule lives here and
// the store wires it up.
//
// The rule is about request volume, not display. Refreshing one account
// costs four calls — balances, positions, orders, fills — and behind a real
// broker those are four venue requests. A screen showing a single account
// has no business asking about the other nine every five seconds, and this
// project has already had a machine IP-banned by a venue for asking too
// often.

/** How much of the portfolio a watching screen needs. */
export type TradeWatchScope =
	/** Only the account the screen is acting on. */
	| 'active'
	/** Every visible account, because the screen shows every visible one. */
	| 'all';

/** Tracks what the current watchers between them need.
 *
 * Reference counted rather than a flag: two screens can watch at once, and
 * the narrow one must not cancel the broad one's need when it unmounts.
 */
export class WatchScopes {
	private broad = 0;

	/** Registers a watcher, returning its release function. Releasing twice
	 * is a no-op, so a component whose cleanup runs twice cannot
	 * double-decrement and leave the count wrong for everyone else. */
	add(scope: TradeWatchScope): () => void {
		if (scope === 'all') this.broad++;
		let released = false;
		return () => {
			if (released) return;
			released = true;
			if (scope === 'all') this.broad--;
		};
	}

	/** The widest scope any current watcher asked for.
	 *
	 * Read fresh on every tick rather than fixed when the timer started: a
	 * screen mounted or unmounted since then must change what the next tick
	 * fetches. */
	current(): TradeWatchScope {
		return this.broad > 0 ? 'all' : 'active';
	}
}
