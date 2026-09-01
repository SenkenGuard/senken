// Reference counting for `WsClient`'s subscribed topics,
// pulled out of `websocket.ts` as its own pure, framework- and
// network-independent unit so reference-count behavior is testable without a socket
// isolates its own tricky bit (the leader/follower guard) instead of
// leaving it inline where only a real chart library could exercise it.
//
// The property this exists to hold: more than one caller can lease the same
// topic over the one shared WebSocket connection (a chart pane and a
// watchlist row can easily name the same instrument) — mirroring
// `senken_subscription::SubscriptionPool`'s own `LeaseRecord.count`, the
// server only unsubscribes a topic on its *last* lease, so this counter
// must agree: a topic's real `subscribe`/`unsubscribe` frame is due only on
// the first `retain`/the last matching `release`.

export class TopicRefCounter {
	private readonly counts = new Map<string, number>();

	/** Takes one reference on `topic`. Returns `true` exactly when this was
	 * the first outstanding reference — the caller's cue to actually send a
	 * `subscribe` frame. */
	retain(topic: string): boolean {
		const count = (this.counts.get(topic) ?? 0) + 1;
		this.counts.set(topic, count);
		return count === 1;
	}

	/** Releases one reference on `topic`. Returns `true` exactly when that
	 * was the last outstanding reference — the caller's cue to actually send
	 * an `unsubscribe` frame. A release with no matching retain (count
	 * already zero, or `topic` never seen) is a no-op that reports `false`,
	 * the same "nothing left to release" shape
	 * `senken_subscription::Actor::release` falls back to for its own
	 * unreachable-in-practice case. */
	release(topic: string): boolean {
		const current = this.counts.get(topic);
		if (current === undefined || current <= 0) return false;
		if (current > 1) {
			this.counts.set(topic, current - 1);
			return false;
		}
		this.counts.delete(topic);
		return true;
	}

	/** Every topic with at least one outstanding reference — what a fresh
	 * connection must replay `subscribe` for on reconnect. */
	topics(): IterableIterator<string> {
		return this.counts.keys();
	}
}
