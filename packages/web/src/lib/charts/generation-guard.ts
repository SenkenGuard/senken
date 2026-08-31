// A monotonically increasing token marking whichever run of an async effect
// is "current." `begin()` bumps it and hands back the token that run must
// present in every later continuation; `isCurrent(token)` says whether that
// run is still the one allowed to write shared state.
//
// `chart-pane.svelte`'s bars effect already used this exact shape ad hoc
// (`loadToken`, bumped on entry and compared in every `.then()`). The
// indicator effect had no equivalent — its only guard was `instrument`/
// `spec` equality, which does not change on a layer being shown, hidden, or
// edited, so two rapid toggles produced two overlapping runs that both
// passed and then fought over the same series map. Lifting the pattern out
// once, rather than re-deriving it a second time by hand, is what makes it
// unit-testable without the Svelte compiler — same reasoning as
// `once-guard.ts`.
export class GenerationGuard {
	private current = 0;

	/** Marks a new run as current and returns the token it must keep
	 * presenting to `isCurrent`. */
	begin(): number {
		return ++this.current;
	}

	/** Whether `token` (from a prior `begin()`) still names the current run
	 * — `false` once a later `begin()` has superseded it. */
	isCurrent(token: number): boolean {
		return token === this.current;
	}
}
