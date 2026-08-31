// What the new-alert form's instrument/timeframe fields should show right
// now, given whether the form is open and the pane it was opened from.
// Pulled out of `alerts-panel.svelte` as a plain function, the same reason
// `workspace-error.ts` pulls `layoutMutationErrorMessage` out of
// `workspace-store.svelte.ts`: a `.svelte` file's `$state`/`$effect` need
// the Svelte compiler to even load, so the actual decision this component
// makes lives here, where a plain `bun test` can exercise it directly and
// prove the property that matters — that the answer is recomputed from the
// pane's *current* instrument/timeframe every time, never remembered from
// whichever pane happened to be active when the form was first opened.
export interface PaneDefaults {
	instrument: string;
	timeframe: string;
}

/** `null` while the form is closed (nothing to prefill — the fields keep
 * whatever they last held, inert until reopened). While open, always
 * exactly `pane`'s current values: an alert form must never keep offering a
 * symbol the user is no longer looking at, so this is a plain projection of
 * `pane`, not a value captured once and reused. */
export function alertFormPrefill(showCreate: boolean, pane: PaneDefaults): PaneDefaults | null {
	return showCreate ? pane : null;
}
