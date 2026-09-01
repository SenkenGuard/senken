// Renders the real, compiled component through Svelte's own server renderer
// (`svelte/server`) and reads the actual output markup — the same `data-*`
// attributes and text a browser would show — rather than asserting on the
// logic that is supposed to produce them. See
// `../../../scripts/svelte-server-preload.ts` for how a `.svelte` import works
// inside `bun test`.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import HistoryEdgeStatus from './history-edge-status.svelte';

describe('history edge status (real DOM output)', () => {
	test('idle renders nothing', () => {
		const { body } = render(HistoryEdgeStatus, { props: { edgeState: 'idle' } });
		expect(body).not.toContain('data-history-edge-state');
	});

	test('loading, error and full are three distinct states with three distinct labels', () => {
		const loading = render(HistoryEdgeStatus, { props: { edgeState: 'loading' } }).body;
		const error = render(HistoryEdgeStatus, { props: { edgeState: 'error' } }).body;
		const full = render(HistoryEdgeStatus, { props: { edgeState: 'full' } }).body;

		expect(loading).toContain('data-history-edge-state="loading"');
		expect(loading).toContain('LOADING EARLIER HISTORY');

		expect(error).toContain('data-history-edge-state="error"');
		expect(error).toContain('EARLIER HISTORY COULD NOT LOAD');

		expect(full).toContain('data-history-edge-state="full"');
		expect(full).toContain('PANE HISTORY LIMIT REACHED');

		const labels = new Set(
			[loading, error, full].map((body) => body.replace(/<[^>]+>/g, '').trim())
		);
		expect(labels.size).toBe(3);
	});

	// The reader needs to see *where* history starts, and a badge floating over
	// the middle of the plot cannot say that — it covers the candles it is
	// describing and discards the only information that mattered. The mark is
	// drawn on the chart at the earliest bar instead
	// (`$lib/charts/history-marks-primitive`), so this component must stay out
	// of it entirely.
	test('the start of history is never rendered as a floating notice', () => {
		const { body } = render(HistoryEdgeStatus, { props: { edgeState: 'end' } });
		expect(body).not.toContain('data-history-edge-state');
		expect(body.replace(/<[^>]+>/g, '').trim()).toBe('');
	});
});
