<script lang="ts">
	// One sub-pane indicator's own lightweight-charts canvas. Which
	// indicators live here is `INDICATOR_CATALOG`'s own `subPane` flag —
	// MACD, RSI, Stochastic and ATR. Mounted below the main
	// chart, in its own resizable strip (pane-cell.svelte), exactly the way
	// the reference gives each `subPanes` entry its own `data-sub-pane` mount
	// point — separate from
	// chart-pane.svelte's overlay rendering because a sub-pane indicator has
	// its own y-axis scale entirely (an RSI's 0-100 has nothing to do with
	// price), so it cannot share chart-pane's price-scale chart instance.
	//
	// values come from `$lib/charts/bars.ts`'s
	// `loadIndicatorSeries` (`POST /api/indicators/compute`), never the
	// deleted `$lib/indicators/browser-compute.ts`.
	import { onMount } from 'svelte';
	import { createChart, type IChartApi, type ISeriesApi, type UTCTimestamp } from 'lightweight-charts';
	import { defaultBarWindow, indicatorFieldScale, isIntradaySpec, timeAxisFormatter } from './chart-config';
	import { loadIndicatorSeries } from '$lib/charts/bars';
	import { plotsForLayer, LINE_STYLE_MAP } from '$lib/charts/layer-style';
	import { alignToBarTimes } from '$lib/charts/align-to-bars';
	import type { LayerRuntime } from '$lib/charts/pane-runtime';
	import type { ChartSettings } from '$lib/mock/chart-settings';
	import { chartPaneThemeColors, isDarkTheme } from './chart-theme';

	let {
		instrument,
		spec,
		crosshairTime,
		onCrosshairTime,
		layer,
		settings,
		priceScale,
		barTimes,
		onChartApi,
		onApplyPlots
	}: {
		instrument: string;
		spec: string;
		/** A crosshair time coming from a sibling pane. */
		crosshairTime?: UTCTimestamp | null;
		/** Where this pane's own crosshair sits, for the siblings. */
		onCrosshairTime?: (time: UTCTimestamp | null) => void;
		layer: LayerRuntime;
		/** The pane's settings, so this axis reads the same as the main
		 * chart's above it. */
		settings: ChartSettings;
		/** The pane's own `price_scale`, from the same `GET /api/bars/range`
		 * call chart-pane.svelte already made — an Atr/Macd sub-pane value is
		 * still price-scaled (see `chart-config.ts`'s `indicatorFieldScale`
		 * doc), so it needs this even though it never renders a candle. */
		priceScale: number;
		/** The bars the main chart above is showing, in its order. Every plot
		 * here is drawn on exactly these, gaps included, so the two charts
		 * share a logical index — see `$lib/charts/align-to-bars.ts`. This is
		 * also the replay window: the main chart has already cut it. */
		barTimes: UTCTimestamp[];
		/** Fires with this sub-pane's `IChartApi` once created, and with
		 * `undefined` just before it is torn down — see chart-pane.svelte's
		 * identical prop; `pane-cell.svelte` links the two together so this
		 * sub-pane's x-axis never drifts from the main chart above it. */
		onChartApi?: (chart: IChartApi | undefined) => void;
		/** Hands this strip's whole data update to the pane to run.
		 *
		 * The library re-anchors a chart's visible range when it receives a
		 * new series and announces that like an ordinary gesture — from
		 * inside the data call — so an indicator finishing its load would
		 * otherwise decide where the entire pane is scrolled to. The pane
		 * runs this with that announcement suppressed. Handing over the
		 * update rather than reporting afterwards is the point: afterwards
		 * is already too late. */
		onApplyPlots?: (apply: () => void) => void;
	} = $props();

	let container: HTMLDivElement | undefined = $state();
	let chart: IChartApi | undefined;
	let ro: ResizeObserver | undefined;
	let mo: MutationObserver | undefined;
	const series = new Map<string, ISeriesApi<'Line'> | ISeriesApi<'Histogram'>>();
	// See chart-pane.svelte's identical field for why this exists: a
	// sub-pane strip appearing/disappearing (the last one being removed, or
	// this being the first one added) flips pane-cell.svelte's `{#if}`
	// branch and destroys/recreates this component while a
	// `loadIndicatorSeries` call is still in flight.
	let destroyed = false;

	function applyTheme() {
		if (!chart) return;
		const T = chartPaneThemeColors(isDarkTheme());
		chart.applyOptions({
			layout: { background: { color: T.bg }, textColor: T.axis, fontFamily: "'IBM Plex Mono', monospace", fontSize: 9 },
			grid: { vertLines: { color: T.grid }, horzLines: { color: T.grid } },
			rightPriceScale: { borderColor: T.border },
			timeScale: {
				borderColor: T.border,
				rightOffset: 6,
				timeVisible: isIntradaySpec(spec),
				secondsVisible: false
			},
			crosshair: {
				mode: 0,
				vertLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label },
				horzLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label }
			}
		});
	}

	onMount(() => {
		if (!container) return;
		const T = chartPaneThemeColors(isDarkTheme());
		chart = createChart(container, {
			layout: { background: { color: T.bg }, textColor: T.axis, fontFamily: "'IBM Plex Mono', monospace", fontSize: 9 },
			grid: { vertLines: { color: T.grid }, horzLines: { color: T.grid } },
			rightPriceScale: { borderColor: T.border },
			timeScale: {
				borderColor: T.border,
				rightOffset: 6,
				visible: true,
				timeVisible: isIntradaySpec(spec),
				secondsVisible: false
			},
			crosshair: {
				mode: 0,
				vertLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label },
				horzLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label }
			},
			width: container.clientWidth,
			height: container.clientHeight
		});
		onChartApi?.(chart);

		chart.subscribeCrosshairMove((param) => {
			onCrosshairTime?.((param.time as UTCTimestamp | undefined) ?? null);
		});

		ro = new ResizeObserver(() => {
			if (!container || !chart) return;
			chart.applyOptions({ width: container.clientWidth, height: container.clientHeight });
		});
		ro.observe(container);

		mo = new MutationObserver(applyTheme);
		mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });

		return () => {
			destroyed = true;
			ro?.disconnect();
			mo?.disconnect();
			series.clear();
			// Before `chart.remove()` — see chart-pane.svelte's identical note.
			onChartApi?.(undefined);
			try {
				chart?.remove();
			} catch {
				// already gone
			}
		};
	});

	// Full reconcile on every relevant change, same reasoning as
	// chart-pane.svelte's overlay effect: this only ever re-runs on an
	// add/remove/edit that already re-renders the component.
	// The pointer is in another pane: draw the same vertical line here so one
	// pointer reads as one position across the whole cell. The price handed
	// to `setCrosshairPosition` is deliberately off-canvas — the API needs
	// one, but a horizontal line belongs only to the pane the pointer is
	// actually in.
	$effect(() => {
		if (!chart) return;
		if (crosshairTime == null) {
			chart.clearCrosshairPosition();
			return;
		}
		const anchorSeries = series.values().next().value;
		if (!anchorSeries) return;
		const offscreen = anchorSeries.coordinateToPrice(-40);
		if (offscreen == null) return;
		chart.setCrosshairPosition(offscreen, crosshairTime, anchorSeries);
	});

	// The axis must relabel when the timeframe changes, not only when this
	// sub-pane is first built — the same reason the main pane re-applies it.
	$effect(() => {
		if (!chart) return;
		chart.applyOptions({
			timeScale: {
				timeVisible: isIntradaySpec(spec),
				secondsVisible: false,
				tickMarkFormatter: timeAxisFormatter(spec, settings.dayOfWeek, settings.timeFormat)
			}
		});
	});

	$effect(() => {
		if (!chart || !layer.visible || !layer.indicatorName) return;
		const currentInstrument = instrument;
		const currentSpec = spec;
		const indicatorName = layer.indicatorName;
		const params = layer.params;
		// Read synchronously so this effect re-runs once the real
		// `price_scale` arrives from the parent — see chart-pane.svelte's
		// identical comment on why reading it inside the `.then()` below
		// would silently plot Atr/Macd's price-scaled fields undivided on a
		// fresh mount.
		const currentPriceScale = priceScale;
		const { from, to } = defaultBarWindow(currentSpec);
		// Nothing is drawn before the main chart has bars: a plot alone on an
		// empty strip has no bars to be read against, and there is no
		// timeline yet to align it to.
		const timeline = barTimes;
		if (timeline.length === 0) return;

		loadIndicatorSeries(currentInstrument, currentSpec, from, to, indicatorName, params)
			.then(({ byField }) => {
				if (destroyed || !chart || currentInstrument !== instrument || currentSpec !== spec) return;
				// Captured before the closure below: the update is handed to
				// the pane to run, and a narrowed `chart` does not survive
				// being read from inside a callback.
				const activeChart = chart;
				const plots = plotsForLayer(layer.id, indicatorName);
				const seen = new Set<string>();
				const applyPlots = () => {
				for (const plot of plots) {
					const points = byField.get(plot.field) ?? [];
					if (points.length === 0) continue;
					seen.add(plot.field);
					const scaleKind = indicatorFieldScale(indicatorName, plot.field);
					const divisor = scaleKind === 'price' ? 10 ** currentPriceScale : 1;
					let handle = series.get(plot.field);
					if (!handle) {
						handle = plot.type === 'histogram' ? activeChart.addHistogramSeries({ priceLineVisible: false }) : activeChart.addLineSeries({ priceLineVisible: false, lastValueVisible: true });
						series.set(plot.field, handle);
					}
					handle.applyOptions(
						plot.type === 'line'
							? { color: plot.color, lineWidth: plot.width as 1 | 2 | 3 | 4, lineStyle: LINE_STYLE_MAP[plot.style], visible: plot.visible }
							: { color: plot.color, visible: plot.visible }
					);
					handle.setData(
						alignToBarTimes(
							timeline,
							points.map((p) => ({ time: p.time, value: p.value / divisor }))
						)
					);
				}
				for (const [field, handle] of series) {
					if (!seen.has(field)) {
						try {
							activeChart.removeSeries(handle);
						} catch {
							// chart already torn down
						}
						series.delete(field);
					}
				}
				};
				if (onApplyPlots) onApplyPlots(applyPlots);
				else applyPlots();
			})
			.catch(() => {
				// leaves whatever was last rendered; a transient failure here
				// should not tear the sub-pane down.
			});
	});
</script>

<div bind:this={container} class="absolute inset-0"></div>
