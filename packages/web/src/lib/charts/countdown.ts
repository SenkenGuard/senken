// Pure countdown maths for the price-axis "time to next bar" primitive
// (`price-badge-primitive.ts`) — kept free of `lightweight-charts` and
// `setInterval` so both are unit-testable with a plain `bun test`.

/** Advances `deadlineMs` by whole `stepMs` increments until it is back in
 * the future relative to `nowMs`. This is only the *local* half of the
 * countdown's deadline: the server is the only one that computes a bucket
 * *boundary* (`BarRangeResponse.next_bar_open_at`, from `senken-series`'
 * own anchor — a UTC-naive guess is wrong for a venue-anchored daily-or-
 * coarser series), but nothing re-asks it every second between bars
 * reloads, so once the countdown reaches zero this is what rolls that
 * *same*, already-confirmed boundary forward by one whole bar step — never
 * a new boundary computed from scratch, only the one the server already
 * gave, advanced by a duration the server also already confirmed
 * (`TF_DURATION_SECONDS`). Handles more than one elapsed step at once (a
 * backgrounded tab, a slow tick) rather than only ever adding one. */
export function advanceDeadline(deadlineMs: number, stepMs: number, nowMs: number): number {
	if (stepMs <= 0) return deadlineMs;
	let deadline = deadlineMs;
	while (nowMs >= deadline) deadline += stepMs;
	return deadline;
}

/** The price-axis label's exact text: `"12:07"` under an hour remaining,
 * `"1:02:07"` at or past one hour. Negative input (a deadline momentarily
 * behind "now", before the next tick's `advanceDeadline` catches it up)
 * clamps to zero rather than showing a negative countdown. */
export function formatCountdown(remainingMs: number): string {
	const totalSeconds = Math.max(0, Math.ceil(remainingMs / 1000));
	const hours = Math.floor(totalSeconds / 3600);
	const minutes = Math.floor((totalSeconds % 3600) / 60);
	const seconds = totalSeconds % 60;
	const pad = (n: number): string => String(n).padStart(2, '0');
	return hours > 0 ? `${hours}:${pad(minutes)}:${pad(seconds)}` : `${minutes}:${pad(seconds)}`;
}
