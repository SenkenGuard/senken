// The pure part of cross-pane chart sync (TIME/CROSSHAIR in the layout
// menu's "SYNC IN LAYOUT" group): given which pane raised an update, which
// other panes should receive it. The wiring itself — subscribing to
// lightweight-charts' own `subscribeVisibleLogicalRangeChange`/
// `subscribeCrosshairMove` and guarding against the fan-out re-entering
// itself — lives in `routes/charts/+page.svelte`, which is the one place
// that holds every pane's `IChartApi`; this file exists so the fan-out set
// itself is testable without a chart.

/** Every pane index in `[0, paneCount)` other than `sourceIndex` — the set
 * of panes a change raised on `sourceIndex` should be applied to. */
export function syncTargets(sourceIndex: number, paneCount: number): number[] {
	const targets: number[] = [];
	for (let i = 0; i < paneCount; i++) {
		if (i !== sourceIndex) targets.push(i);
	}
	return targets;
}
