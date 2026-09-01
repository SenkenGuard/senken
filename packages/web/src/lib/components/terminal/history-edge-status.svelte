<script lang="ts">
	// The pane's *transient* statements about loading history: a request is in
	// flight, a request failed, or the pane is holding all the bars it may.
	//
	// What is deliberately not here: where history starts, and where the data
	// is missing. Those are facts about a *place on the time axis*, not about
	// something happening now — a reader needs to see where, not merely that,
	// and a badge floating over the middle of the plot both hides the candles
	// it describes and throws that position away. They are drawn on the chart
	// itself by `$lib/charts/history-marks-primitive`, at the times they are
	// about, and they pan and zoom with the bars.
	//
	// Split out of `chart-pane.svelte` so this stays a plain, dependency-free
	// component a test can render and read attributes off directly. See
	// `history-edge-status.test.ts`.
	import type { HistoryEdgeState } from '$lib/charts/chart-window';

	let {
		edgeState
	}: {
		edgeState: HistoryEdgeState;
	} = $props();

	const edgeLabel = $derived.by((): string | null => {
		if (edgeState === 'loading') return 'LOADING EARLIER HISTORY…';
		if (edgeState === 'error') return 'EARLIER HISTORY COULD NOT LOAD · PAN LEFT TO RETRY';
		if (edgeState === 'full')
			return 'PANE HISTORY LIMIT REACHED · JUMP TO A DATE TO LOOK FURTHER BACK';
		return null;
	});
</script>

<!-- Bottom-centre, above the time axis — the anchor `chart-pane.svelte`'s own
     progress notice already argues for, and the one place on this edge that is
     free of the chart's attribution corner. -->
{#if edgeLabel}
	<div
		class="pointer-events-none absolute inset-x-0 bottom-7 z-[9] flex flex-col items-center"
		data-history-edge-state={edgeState}
	>
		<span class="border border-ink/14 bg-bg2 px-2 py-1 font-mono text-[8px] tracking-[0.14em] text-secondary-foreground">{edgeLabel}</span>
	</div>
{/if}
