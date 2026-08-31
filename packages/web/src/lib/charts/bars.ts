// Bars and indicator values for one pane, through `apiClient` — replaces `$lib/mock/charts.ts`'s `genCandles` and
// `$lib/indicators/browser-compute.ts` entirely; neither is imported here
// or anywhere downstream of this module.
//
// `plan()` runs before `ensure()` whenever a range is not already covered
// (the brief): `onProgress` reports the plan's
// `missing`/`estimate_secs` immediately, so a caller can render "fetching
// ~12s of history" instead of a chart that just sits blank while a job
// runs. `ensure()`'s job is then polled until `"done"` (or a safety
// timeout, in case a venue is unreachable) before the actual `range()` read
// — the read path must cost nothing on a second call for
// the same range, since it never fetches itself.
import { apiClient } from '$lib/api/client';
import type { BarDto, BarJobDto, BarsRequirementDto } from '$lib/api/types';

export interface ResolvedBars {
	bars: BarDto[];
	missing: { from: number; to: number }[];
	priceScale: number;
	qtyScale: number;
	/** `BarRangeResponse.next_bar_open_at`: Unix nanoseconds for the
	 * instant the currently-forming bar closes, computed server-side by
	 * `senken-series` — the one crate that knows this series' anchor. The
	 * client (`price-badge-primitive.ts`) counts down to this value and never
	 * derives a bucket boundary itself: a UTC-anchored guess is wrong for a
	 * venue-anchored Day-or-coarser series. */
	nextBarOpenAt: number;
}

export type BarLoadProgress =
	| { phase: 'checking' }
	| { phase: 'fetching'; requirement: BarsRequirementDto; job: BarJobDto | null }
	| { phase: 'ready' }
	| { phase: 'error'; message: string };

const POLL_INTERVAL_MS = 200;
/** A venue that never responds must not hang the chart forever — after
 * this, `loadBars` gives up polling and reads back whatever `range()`
 * already has (which may still be partial; `ResolvedBars.missing` reports
 * that honestly rather than pretending the range is complete). */
const POLL_TIMEOUT_MS = 45_000;

async function pollJob(jobId: string, onProgress: (job: BarJobDto) => void, shouldContinue: () => boolean): Promise<void> {
	const deadline = Date.now() + POLL_TIMEOUT_MS;
	for (;;) {
		if (!shouldContinue()) return;
		const job = await apiClient.barJobStatus(jobId);
		if (!shouldContinue()) return;
		onProgress(job);
		if (job.phase === 'done' || Date.now() > deadline) return;
		await new Promise((resolve) => setTimeout(resolve, POLL_INTERVAL_MS));
	}
}

/** Resolves `[from, to)` for `instrument`/`spec`, fetching only what is
 * actually missing. Safe to call repeatedly for the same
 * range — the second call's `planBars` reports nothing missing, so it skips
 * straight to `rangeBars` with no `ensureBars` call at all.
 *
 * `shouldContinue`, when given, is checked before every poll tick — a
 * caller whose own effect re-ran (a newer `loadBars` call superseding this
 * one) should return `false` from it so this call's polling loop actually
 * stops instead of continuing to hit `GET /api/bars/jobs/{id}` in the
 * background for up to `POLL_TIMEOUT_MS` after nothing is listening to its
 * result any more. */
export async function loadBars(
	instrument: string,
	spec: string,
	from: number,
	to: number,
	onProgress?: (progress: BarLoadProgress) => void,
	shouldContinue: () => boolean = () => true
): Promise<ResolvedBars> {
	onProgress?.({ phase: 'checking' });
	try {
		const requirement = await apiClient.planBars(instrument, spec, from, to);
		if (requirement.missing.length > 0 && shouldContinue()) {
			onProgress?.({ phase: 'fetching', requirement, job: null });
			const { job_id } = await apiClient.ensureBars({ instrument, spec, from, to });
			await pollJob(job_id, (job) => onProgress?.({ phase: 'fetching', requirement, job }), shouldContinue);
		}
		const range = await apiClient.rangeBars(instrument, spec, from, to);
		onProgress?.({ phase: 'ready' });
		return {
			bars: range.bars,
			missing: range.missing.map((r) => ({ from: r.from, to: r.to })),
			priceScale: range.price_scale,
			qtyScale: range.qty_scale,
			nextBarOpenAt: range.next_bar_open_at
		};
	} catch (error) {
		const message = error instanceof Error ? error.message : 'Could not load bars.';
		onProgress?.({ phase: 'error', message });
		throw error;
	}
}

/** One bar's indicator output already keyed by field, and converted to a
 * `time`/`value` point per field — the shape `chart-pane.svelte`/
 * `sub-pane-chart.svelte` feed straight into a lightweight-charts series.
 * `senken-indicators` never reports a warm-up value, so every
 * point returned here is already real. */
export interface IndicatorSeriesPoint {
	time: number;
	value: number;
}

/** Computes one indicator layer's series for a pane's own instrument/
 * timeframe (`senken-indicators`, server-side, never the browser). Requires the same range to already be `loadBars`-ensured — this
 * mirrors `POST /api/indicators/compute`'s own contract (it resolves bars,
 * it does not fetch them). Returns one array of points per reported field
 * (e.g. two for Macd's `macd_line`/`macd_signal`, before `macd_histogram`),
 * keyed by field name exactly as `crates/api/src/indicator_handlers.rs`'s
 * `field_key` spells it (`"value"`, `"macd_line"`, `"bollinger_upper"`, …).
 */
/** The bar a pane is still building from live ticks, as scaled integers at
 * the instrument's own price scale — the wire form the server expects. */
export interface ProvisionalBar {
	ts_open: number;
	open: number;
	high: number;
	low: number;
	close: number;
	/** Volume accumulated from the ticks seen in this interval, at the
	 * instrument's quantity scale. */
	volume: number;
}

export async function loadIndicatorSeries(
	instrument: string,
	spec: string,
	from: number,
	to: number,
	indicatorName: string,
	params: Record<string, number>,
	provisional?: ProvisionalBar
): Promise<{ byField: Map<string, IndicatorSeriesPoint[]>; missing: { from: number; to: number }[] }> {
	const response = await apiClient.computeIndicator({
		instrument,
		spec,
		from,
		to,
		indicator: { name: indicatorName, params: JSON.stringify(params) },
		...(provisional ? { provisional } : {})
	});
	const byField = new Map<string, IndicatorSeriesPoint[]>();
	for (const point of response.points) {
		for (const fieldValue of point.values) {
			const series = byField.get(fieldValue.field) ?? [];
			series.push({ time: Math.floor(point.ts_open / 1_000_000_000), value: fieldValue.value });
			byField.set(fieldValue.field, series);
		}
	}
	return { byField, missing: response.missing.map((r) => ({ from: r.from, to: r.to })) };
}
