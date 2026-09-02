// Timing rules for trade auto-refresh, pulled out as their own plain,
// framework-free class for the same reason `lib/api/session-funnel.ts`
// sits next to `session.svelte.ts`: `state/trade.svelte.ts` wires real
// browser primitives (`setInterval`, `document.hidden`, `visibilitychange`)
// into this, but the rules themselves — tick on the interval, skip while
// hidden, refresh once on resume, never overlap, keep running past a failed
// tick, ref-count concurrent watchers — are tested here against fake ones,
// so nothing has to sleep a real five seconds to prove any of it.

/** Everything the poller needs from its environment, all replaceable —
 * `TimerHandle` is whatever `setInterval` returns (`ReturnType<typeof
 * setInterval>` in the browser, a plain number in a test double). */
export interface PollerParams<TimerHandle> {
	/** How often to refresh while the tab is visible, in milliseconds. */
	intervalMs: number;
	/** The refresh itself. A rejection is caught (see `runTick`) so one
	 * failed tick can never stop the timer — the timer is the only thing
	 * standing between a stale screen and the next five-second refresh. */
	tick: () => Promise<void>;
	/** Whether the tab is hidden right now. Read fresh on every interval
	 * firing and on every visibility change, never cached. */
	isHidden: () => boolean;
	setInterval: (handler: () => void, ms: number) => TimerHandle;
	clearInterval: (handle: TimerHandle) => void;
	/** Subscribes to visibility changes; returns the unsubscribe function. */
	onVisibilityChange: (handler: () => void) => () => void;
}

/** Starts (or joins) trade auto-refresh; `start()` returns the stop
 * function.
 *
 * Reference counted, in the same shape as `senken_subscription::Lease`:
 * two mounted views share one timer, and the timer stops only once the
 * last caller has released. Returning the stopper rather than exposing a
 * separate `stop()` method is what makes a leaked poller unrepresentable —
 * a caller's `onMount` returning the value `start()` gave it is the whole
 * lifecycle. */
export class TradePoller<TimerHandle> {
	private refCount = 0;
	private timerHandle: TimerHandle | null = null;
	private unsubscribeVisibility: (() => void) | null = null;
	private inFlight = false;

	constructor(private readonly params: PollerParams<TimerHandle>) {}

	/** Joins the poller, starting the timer if this is the first caller.
	 * Returns a stop function; calling it more than once is a no-op, so a
	 * component that calls its own cleanup twice cannot double-release. */
	start(): () => void {
		this.refCount++;
		if (this.refCount === 1) this.activate();
		let released = false;
		return () => {
			if (released) return;
			released = true;
			this.refCount--;
			if (this.refCount === 0) this.deactivate();
		};
	}

	private activate(): void {
		this.timerHandle = this.params.setInterval(() => void this.runTick(), this.params.intervalMs);
		this.unsubscribeVisibility = this.params.onVisibilityChange(() => {
			// Only *becoming* visible has anything to refresh for — a tab
			// returned to after ten minutes must not show a ten-minute-old
			// book for five more seconds. Reading `isHidden()` here rather
			// than tracking the previous state is what tells "became
			// visible" from "became hidden" apart: the event fires for both,
			// but only one of them should read false at this instant.
			if (!this.params.isHidden()) void this.runTick();
		});
	}

	private deactivate(): void {
		if (this.timerHandle !== null) this.params.clearInterval(this.timerHandle);
		this.timerHandle = null;
		this.unsubscribeVisibility?.();
		this.unsubscribeVisibility = null;
	}

	private async runTick(): Promise<void> {
		if (this.params.isHidden()) return;
		if (this.inFlight) return; // never overlapping — a slow adapter must not build a backlog
		this.inFlight = true;
		try {
			await this.params.tick();
		} catch {
			// A failed tick is kept against its own account (`Portfolio.error`,
			// see `portfolio-refresh.ts`) by the tick function itself; this
			// timer must never stop over it.
		} finally {
			this.inFlight = false;
		}
	}
}
