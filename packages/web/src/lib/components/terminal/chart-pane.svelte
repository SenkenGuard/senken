<script lang="ts">
	// One pane's lightweight-charts candlestick canvas (`data-chart-pane` mount points at line 450, chart construction at
	// lines 1736-1801, theme palette at chartTheme() line 1917, replay
	// slicing at applyReplay() line 2088).
	//
	// Drawing objects (horizontal line, trend line, rectangle, ray, Fibonacci
	// retracement, text note) are rendered and hit-tested by
	// `$lib/charts/drawing-primitive.ts`'s
	// `DrawingsPrimitive`, attached to this pane's candlestick series — not
	// the reference's own click-to-paint `IPriceLine`s (line 1780), which
	// have no identity a later click could select or edit. Which tool needs
	// how many clicks, and which of them persists a `DrawingKindTag` at all,
	// comes from `$lib/charts/pane-runtime.ts`'s `TOOL_ANCHOR_COUNT`/
	// `DRAWING_KIND_FOR_TOOL` tables — the measure tool is the one live
	// entry with no `DrawingKindTag` (`$lib/charts/measure.ts`'s pure delta
	// math), so it paints a readout instead of ever calling
	// `onCreateDrawing`.
	//
	// bars come from `$lib/charts/bars.ts`
	// (`GET /api/bars/plan` -> `POST /api/bars/ensure` -> poll -> `GET
	// /api/bars/range`), never `$lib/mock/charts.ts`'s deleted `genCandles`.
	// Overlay indicator series come from the same module's
	// `loadIndicatorSeries` (`POST /api/indicators/compute`), never the
	// deleted `$lib/indicators/browser-compute.ts` — the whole
	// point.
	//
	// lightweight-charts touches the DOM directly (ResizeObserver, canvas),
	// so chart creation happens in onMount, never at module scope — the app
	// runs with `ssr = false` (src/routes/+layout.ts) so this only ever
	// mounts in the browser, but keeping the browser-only work inside
	// onMount/effects (rather than importing side effects at module init)
	// is what actually keeps this safe for the static `vite build` step.
	import { onMount, untrack } from 'svelte';
	import { CandlestickSeries, HistogramSeries, LineSeries, createChart, createTextWatermark, type IChartApi, type ISeriesApi, type ITextWatermarkPluginApi, type LogicalRange, type Time, type UTCTimestamp } from 'lightweight-charts';
	import {
		TF_DURATION_SECONDS,
		type Timeframe,
		type ToolKey,
		isIntradaySpec,
		buildTimeLabelOptions,
		timeLabelOptionsSignature
	} from './chart-config';
	import { userZoneStore } from '$lib/state/user-zone.svelte';
	import { loadBars, loadIndicatorSeries, type BarLoadProgress } from '$lib/charts/bars';
	import { objectDrawablesFromIndicatorDisplay } from '$lib/charts/indicator-display';
	import { fetchIndicatorCatalog } from '$lib/charts/indicator-catalog';
	import { GenerationGuard } from '$lib/charts/generation-guard';
	import { indicatorFieldScale, plotsForLayer, LINE_STYLE_MAP, OVERLAY_INSTRUMENT_PLOT } from '$lib/charts/layer-style';
	import {
		overlayInstrumentCloseSeries,
		overlayInstrumentSignature,
		overlaySeriesKeysToRemove,
		planOverlayInstrumentSeries
	} from '$lib/charts/overlay-instrument';
	import { DRAWING_KIND_FOR_TOOL, TOOL_ANCHOR_COUNT, type DrawingRuntime, type LayerRuntime } from '$lib/charts/pane-runtime';
	import { formatMeasureLabel, measureStats } from '$lib/charts/measure';
	import { DrawingsPrimitive } from '$lib/charts/drawing-primitive';
	import { drawingAt } from '$lib/charts/drawing-layer';
	import {
		indicatorRange,
		initialBarWindow,
		reloadWindow,
		defaultFrame,
		frameIsPending,
		pendingFrameIsLive,
		indexOfTime,
		logicalRangeShift,
		historyLoadPriority,
		HISTORY_PAGE_BARS,
		MAX_HISTORY_BARS,
		previousHistoryPage,
		preserveVisibleRange,
		shouldPrefetchHistory,
		resolveJumpWindow,
		applyHistoryJump,
		historyIsFull,
		type HistoryEdgeState,
		type HistoryGap
	} from '$lib/charts/chart-window';
	import HistoryEdgeStatus from './history-edge-status.svelte';
	import { HistoryMarksPrimitive } from '$lib/charts/history-marks-primitive';
	import { nativePaneData } from '$lib/charts/native-panes';
	import { statusVolumeFromDto, type StatusBar } from '$lib/charts/status-line';
	import { paneStretchFactors, shouldFramePriceScale } from '$lib/charts/pane-settings';
	import { LastPriceBadgePrimitive, type PriceDirection } from '$lib/charts/price-badge-primitive';
	import { deriveLiveState, liveChipLabel, overlayMessage, priceLineColorFor, showCountdown } from '$lib/charts/live-state';
	import type { MarketStatus } from '$lib/charts/live-state';
	import { chartPaneThemeColors, isDarkTheme } from './chart-theme';
	import { defaultChartSettings, type ChartSettings } from '$lib/mock/chart-settings';
	import { wsClient } from '$lib/api/websocket';
	import type { IndicatorSubscribeRequest } from '$lib/api/indicator-topic';
	import { wsEventsStore, priceFromEvent } from '$lib/api/ws-events.svelte';
	import { isUnsupportedFor, quoteFromEvent, indicatorFrameFor } from '$lib/api/ws-frames';
	import { ensureSourcesLoaded, hasLiveFeed, hasQuoteFeed } from '$lib/api/sources.svelte';

	/** A freshly drawn object's starting style — the same default plot colour
	 * `$lib/charts/layer-style.ts` already uses, so a new drawing and a new
	 * indicator plot look consistent on a fresh chart. `visible: true` since
	 * every drawing this pane creates starts out shown. */
	const DEFAULT_DRAWING_STYLE = { color: '#f2f2ef', width: 2, lineStyle: 'SOLID' as const, visible: true };

	/** A freshly placed text note's starting content — there is no text-entry
	 * dialog yet, so a click places a note with placeholder text the user can
	 * still drag, select and delete like any other drawing; only the
	 * one-click path (`TOOL_ANCHOR_COUNT.text === 1`) ever uses these. */
	const DEFAULT_TEXT_NOTE_TEXT = 'NOTE';
	const DEFAULT_TEXT_NOTE_ANCHOR = 'above' as const;

	let {
		instrument,
		marketStatus,
		spec,
		tool,
		replayIdx,
		clearToken,
		overlayLayers,
		subPaneLayers = [],
		drawings,
		selectedDrawingId,
		onStatusBar,
		onNarrow,
		onLastClose,
		onLiveNotice,
		onPriceScale,
		onLivePrice,
		onPaneTops,
		onBarsLoading,
		onBarTimes,
		onChartApi,
		onCreateDrawing,
		onSelectDrawing,
		onToolConsumed,
		onContextMenu,
		onMoveDrawing,
		onLayerLoading,
		onAutoScaleChanged,
		reloadToken = 0,
		resetToken = 0,
		jumpTarget = null,
		settings
	}: {
		instrument: string;
		/** Catalogued session state, if the source supplied one for this instrument. */
		marketStatus?: MarketStatus;
		spec: string;
		tool: ToolKey;
		/** Bar count to reveal during replay, or `null` to show the full series
		 * (applyReplay, line 2088). */
		replayIdx: number | null;
		/** Incrementing counter — bumping it clears every drawing in the
		 * pane (`clearPaneDrawings`). */
		clearToken: number;
		/** Indicator layers placed over the price chart.
		 * Excludes any `indicator_sub_pane` layer; the caller (pane-cell.svelte)
		 * is the one that splits a pane's layers that way. */
		overlayLayers: LayerRuntime[];
		/** Indicator layers rendered in the chart's native secondary panes. */
		subPaneLayers?: LayerRuntime[];
		/** This pane's drawing objects — rendered and hit-tested by
		 * `$lib/charts/drawing-primitive.ts`, not painted as price lines. */
		drawings: DrawingRuntime[];
		/** The id of the drawing currently selected (cursor tool, clicked an
		 * existing object), or `null`. Selection state lives one level up
		 * (`pane-cell.svelte`) since selecting a drawing opens a properties
		 * panel outside this component. */
		selectedDrawingId: string | null;
		/** Fires with the bar the status line should describe: the one under
		 * the crosshair while the pointer is over the plot, and the newest
		 * loaded bar otherwise. The numbers, not a formatted string — which
		 * of them a reader wants to see is six stored settings the header
		 * applies (`$lib/charts/status-line.ts`), and this component has no
		 * business deciding that. `null` only while the pane has no bars. */
		onStatusBar?: (bar: StatusBar | null) => void;
		/** Fires whenever the container crosses the 330px width the reference
		 * uses to hide the venue prefix (paneNarrow, line 1794). */
		onNarrow?: (narrow: boolean) => void;
		/** Fires with the most recent bar's real decimal close, whenever bars
		 * (re)load — the order ticket's `mid` price comes from here now,
		 * instead of a mock candle series. */
		onLastClose?: (price: number) => void;
		/** Fires with the pane's live-state chip: a short label and the
		 * sentence behind it, or `null` while the venue is streaming
		 * normally. Both, because "MARKET CLOSED" and "NO LIVE FEED" are
		 * different facts and a header that prints one of them for the other
		 * is simply wrong. */
		onLiveNotice?: (notice: { label: string; message: string } | null) => void;
		/** Fires with `price_scale` whenever bars (re)load. */
		onPriceScale?: (scale: number) => void;
		/** Fires with the live last-traded price, or `null`
		 * once this pane's instrument changes and no tick has arrived yet
		 * for the new one. `pane-header.svelte` shows this next to the
		 * instrument chip whenever the crosshair isn't overriding it. */
		onLivePrice?: (price: number | null) => void;
		/** Where each sub-pane actually starts, in pixels from the top of
		 * this chart's container. Measured from the panes themselves, not
		 * computed as a percentage: the panes divide the chart area *minus*
		 * the time axis and the separators between them, so a percentage of
		 * the container is never the same number and the header drifts off
		 * the pane it labels. Index 0 is the first sub-pane. */
		onPaneTops?: (tops: number[]) => void;
		/** Whether this pane's bars are being fetched — the initial window, a page
		 * of older history, or a jump. Reported so the instrument row can show it
		 * the same way each layer already shows its own. */
		onBarsLoading?: (loading: boolean) => void;
		/** Fires with the times of the bars this chart is actually showing. */
		onBarTimes?: (times: UTCTimestamp[]) => void;
		/** Fires with this pane's `IChartApi` once created, and with
		 * `undefined` just before it is torn down — `pane-cell.svelte` uses
		 * this to link the main pane's time axis to each of its own sub-pane
		 * indicator charts (main and sub-panes must share an x-axis: panning
		 * or zooming one moves the other). */
		onChartApi?: (chart: IChartApi | undefined) => void;
		/** Fires once a horizontal line, trend line or rectangle is complete
		 * (one click for a horizontal line, two for the other two). */
		onCreateDrawing?: (drawing: Omit<DrawingRuntime, 'id' | 'position'>) => void;
		/** Fires when the cursor tool clicks an existing drawing (its id) or
		 * empty chart space (`null`, clearing the selection). */
		onSelectDrawing?: (id: string | null) => void;
		/** Fires once a drawing finishes, so the toolbar returns to the cursor
		 * tool: after drawing a line the tool returns to the cursor. */
		onToolConsumed?: () => void;
		/** A right-click on this pane, with the pointer position and which
		 * region it landed in. The pane does not own the menu — the page
		 * does, so one menu can serve every pane. */
		onContextMenu?: (event: {
			x: number;
			y: number;
			region: 'chart' | 'price-scale' | 'time-scale';
		}) => void;
		/** Bumping this reloads the pane's bars from scratch — the escape
		 * hatch for a chart that has ended up in a state the user cannot
		 * explain. */
		/** A drawing was dragged to a new position. Fired once, when the drag
		 * ends — not on every mouse move, which would write a layout per
		 * frame. */
		onMoveDrawing?: (
			id: string,
			patch: Partial<Pick<DrawingRuntime, 'price' | 'start' | 'end'>>
		) => void;
		/** Which indicator layers are recomputing right now. The plot already
		 * on screen stays until its replacement arrives — this is only so the
		 * pane header can say a layer is working. */
		onLayerLoading?: (ids: string[]) => void;
		/** The chart turned auto-fit off by itself — the user dragged the
		 * price axis. Reported so the stored setting, the menu's tick and the
		 * axis shortcut all still describe what the chart is actually doing. */
		onAutoScaleChanged?: (autoScale: boolean) => void;
		reloadToken?: number;
		/** Bumping this returns the pane to its default frame. */
		resetToken?: number;
		/** A "go to date" request, keyed by a token so a target the reader
		 * picks twice in a row (same date, e.g. after panning away and back)
		 * still fires — an object identity or timestamp equality check would
		 * silently drop the second request. `null`/`token: 0` means nothing
		 * has been requested yet. */
		jumpTarget?: { token: number; targetNanos: number } | null;
		/** This pane's display settings. `'auto'` and `'THEME'` mean "leave it
		 * to the palette"; anything else is the user's explicit choice. */
		settings?: ChartSettings;
	} = $props();

	let container: HTMLDivElement | undefined = $state();
	let chart: IChartApi | undefined;
	let series: ISeriesApi<'Candlestick'> | undefined;
	let drawingsPrimitive: DrawingsPrimitive | undefined;
	let onPointerDown: ((e: MouseEvent) => void) | undefined;
	let onPointerMove: ((e: MouseEvent) => void) | undefined;
	let onPointerUp: (() => void) | undefined;
	let priceBadge: LastPriceBadgePrimitive | undefined;
	let historyMarks: HistoryMarksPrimitive | undefined;
	let watermark: ITextWatermarkPluginApi<Time> | undefined;
	let ro: ResizeObserver | undefined;
	let mo: MutationObserver | undefined;
	// Watches the pane elements themselves, not this component's container.
	// Dragging the separator between two panes changes their heights while
	// the container keeps exactly the same box, so the container observer
	// never fires and the headers keep the positions they were given
	// before the drag.
	let paneRo: ResizeObserver | undefined;
	let lastNarrow: boolean | undefined;
	// The trend-line/rectangle tools need two clicks — this holds the first
	// point between them. `null` whenever no such drawing is in progress.
	let pendingStart: { time: number; price: number } | null = null;
	// One overlay series per (layer, field) pair — reconciled on every data
	// effect run rather than diffed, since the whole overlay set only ever
	// changes on an add/remove/edit that already re-renders this component.
	const overlaySeries = new Map<string, ISeriesApi<'Line'> | ISeriesApi<'Histogram'>>();
	const subPaneSeries = new Map<string, ISeriesApi<'Line'> | ISeriesApi<'Histogram'>>();
	// One line series per `overlay_instrument` layer, keyed by layer id — kept
	// apart from `overlaySeries` above (which is keyed by `layerId:field` for
	// indicator plots) so the two reconciliation effects never collide over
	// the same map, even though nothing stops a pane from carrying both kinds
	// of layer at once.
	const overlayInstrumentSeries = new Map<string, ISeriesApi<'Line'>>();

	/** One loaded bar. `volume` is carried because the status line can be
	 * asked to show it and there is nowhere else to get it from — the
	 * candlestick series itself never sees it. */
	type LoadedBar = {
		ts_open: number;
		open: number;
		high: number;
		low: number;
		close: number;
		volume: StatusBar['volume'];
	};
	let bars = $state<LoadedBar[]>([]);

	// The range every indicator path must cover: exactly the bars this pane is
	// holding right now, never the window it first asked for. Paging older
	// history prepends to `bars`, and jumping to a date replaces it, so an
	// indicator range derived from the *initial* window stays frozen at its
	// original left edge — candles extend back and the plots stop dead, which
	// is what this pane shipped. `bars` only changes on those two structural
	// events (live ticks go through `liveCandle`), so reading it here does not
	// re-run the fetches on every tick.
	const loadedRange = $derived(
		indicatorRange(bars, TF_DURATION_SECONDS[spec as Timeframe] ?? 3600)
	);
	let priceScale = $state(0);
	let qtyScale = $state(0);
	// The live last-traded price, fed by `wsEventsStore` — see
	// the subscribe/tick effects below.
	let livePrice = $state<number | null>(null);
	// Kept independently from last trade: a quote is its own market-data
	// stream and a new trade must not erase a still-current bid/ask pair.
	let liveQuote = $state<{ bid: number; ask: number } | null>(null);
	// `BarRangeResponse.next_bar_open_at` from the most recent bars load
	// The countdown primitive's deadline. `null` until the first load
	// resolves, and reset on every new instrument/timeframe so a stale
	// deadline from the previous selection never survives onto this one,
	// the same reasoning `bars` itself is cleared for below.
	let nextBarOpenAt = $state<number | null>(null);
	/** The candle currently being built from live ticks, for the live
	 * candlestick draw only — not `$state`, since it is written inside
	 * `untrack` and drawn imperatively via `series.update`. Indicator values
	 * for this same still-forming bar no longer come from a synthetic bar
	 * assembled here: the server's own live indicator session
	 * (`$lib/api/websocket.ts`'s `subscribeIndicator`) computes and streams
	 * them directly, from the same engine `crates/alerts/src/bar_builder.rs`
	 * already used server-side — building that shape a second time here was
	 * the duplication this component no longer does. */
	let liveCandle: {
		ts_open: number;
		open: number;
		high: number;
		low: number;
		close: number;
	} | null = null;
	/** Bumped when a bar closes, so the loader is asked for the finished bar
	 * the venue actually recorded. */
	let barEpoch = $state(0);
	/** The bar a catch-up reload has already been asked for. A venue that
	 * simply has not printed a bar for a while would otherwise get one
	 * request per tick, forever, since every one of them finds the same
	 * still-missing bar. */
	let catchUpRequestedFor = 0;
	/** The `instrument|spec` the loaded bars belong to. */
	let loadedIdentity = '';
	// Set once this pane's subscribed topic gets back an explicit
	// `unsupported` WS frame rather than a `price` tick — the authoritative
	// half of `liveState` below; `hasLiveFeed` (the `GET /api/sources`
	// cache) is the other, checked before a subscribe even goes out.
	let unsupported = $state(false);
	let progress = $state<BarLoadProgress>({ phase: 'checking' });
	/** Bumped by `applyTheme` so the data effect repaints bars whose colour is
	 * baked into the data itself. */
	let themeEpoch = $state(0);
	// A left-edge request is deliberately independent from the primary load
	// generation. Panning into history must not cancel the bars currently on
	// screen, and its `prefetch` priority leaves visible work ahead of it.
	let loadingOlder = $state(false);
	let olderHistoryError = $state<string | null>(null);
	let earliestAvailable = $state<number | null>(null);
	let historyEdgeState = $state<HistoryEdgeState>('idle');
	// The nearest range the server reported genuinely missing
	// (`BarRangeResponse.missing`) from the currently loaded window — not
	// "not fetched yet". Shown by `HistoryEdgeStatus` so a hole in the
	// venue's own history reads as a stated fact instead of a silently
	// continuous line. Replaced (not merged) whenever the loaded window is
	// itself replaced (a fresh load, a date jump); appended alongside older
	// history that is prepended to it.
	let gapRanges = $state<HistoryGap[]>([]);
	const currentGap = $derived(gapRanges[0] ?? null);
	const loadGeneration = new GenerationGuard();
	// The indicator effect's own generation guard — `loadGeneration`
	// only covers bars, and the overlay reconciliation below races
	// independently of it: a layer hidden/shown/edited begins a new
	// generation here without touching `loadGeneration` at all, so a stale,
	// still-resolving indicator fetch can be told apart from the current one.
	const indicatorGeneration = new GenerationGuard();
	const subPaneGeneration = new GenerationGuard();
	/** Indicator names `GET /api/indicators` currently reports — the ten
	 * built-ins (always present) plus every dynamic indicator loaded from
	 * an uploaded `.wasm` component that is not currently disabled.
	 * Refreshed on mount and periodically after (`refreshIndicatorCatalog`
	 * in `onMount`), so a plugin someone disables or re-enables elsewhere
	 * is picked up here without reloading this chart. A layer whose
	 * `indicatorName` is outside this set is excluded from both batch
	 * reconciliation effects below — never removed, never mutated, just
	 * left out of the fetch — which is what turns it into a placeholder:
	 * its plot disappears through the exact same "not seen -> removed"
	 * cleanup a deleted layer already goes through, and the moment its
	 * name reappears here the same effects pick it back up with its
	 * stored `params` untouched. Reassigned wholesale on every refresh
	 * (never mutated in place), the same convention `liveIndicatorTopics`
	 * below uses.
	 */
	let indicatorCatalog = $state<Set<string>>(new Set());
	// The overlay-instrument reconciliation effect's own generation guard —
	// same reasoning as `indicatorGeneration` just above: a layer
	// hidden/shown/added/removed begins a new generation here without
	// touching the indicator effect's guard at all, so a stale, still-
	// resolving `loadBars` call for a removed overlay instrument can be told
	// apart from the current one.
	const overlayInstrumentGeneration = new GenerationGuard();
	// Set once this component's `onMount` cleanup runs. Both async effects
	// below (bars, overlay indicators) resolve well after they start, and
	// `pane-cell.svelte`'s `{#if split.sub.length === 0}`/`{:else}` branch
	// switch (adding or removing the *first* sub-pane layer) destroys and
	// recreates this whole component — without this guard, a promise still
	// in flight from the just-destroyed instance would resolve into its own
	// closed-over (removed) `chart`, plotting a stale series onto a chart
	// nothing is showing and corrupting the next instance's autoscale the
	// moment lightweight-charts reuses the freed canvas/context. Every
	// `.then()` below checks this before touching `chart`/`series`.
	let destroyed = false;

	let lastPaneTops = '';
	let observedPanes: HTMLElement[] = [];
	/** The stretch factors last written, so a re-run that would write the
	 * same ones leaves a dragged separator alone. */
	let appliedPaneWeights = '';
	/** Reports every sub-pane's real top edge. Runs on the next frame because
	 * stretch factors are applied to the chart before the browser has laid the
	 * panes out again; measuring in the same tick reads the previous layout. */
	function reportPaneTops(): void {
		if (!chart || !container) return;
		requestAnimationFrame(() => {
			if (destroyed || !chart || !container) return;
			const base = container.getBoundingClientRect().top;
			const tops = chart
				.panes()
				.slice(1)
				.map((pane) => {
					const element = pane.getHTMLElement();
					return element ? Math.round(element.getBoundingClientRect().top - base) : 0;
				});
			observePaneElements();
			const signature = tops.join(',');
			if (signature === lastPaneTops) return;
			lastPaneTops = signature;
			onPaneTops?.(tops);
		});
	}

	/** Points the pane observer at the panes that exist right now. Panes come
	 * and go with the layer set, so the observed elements are re-synced on
	 * every measurement rather than attached once. */
	function observePaneElements(): void {
		if (!chart) return;
		const elements = chart
			.panes()
			.map((pane) => pane.getHTMLElement())
			.filter((element): element is HTMLElement => element != null);
		const unchanged =
			elements.length === observedPanes.length &&
			elements.every((element, index) => element === observedPanes[index]);
		if (unchanged) return;
		observedPanes = elements;
		paneRo?.disconnect();
		for (const element of elements) paneRo?.observe(element);
	}

	function requestOlderHistory(): void {
		if (loadingOlder || destroyed || bars.length === 0) return;
		if (historyIsFull(bars.length)) {
			historyEdgeState = 'full';
			return;
		}
		const first = bars[0];
		const second = bars[1];
		if (!first || !second) return;
		if (earliestAvailable != null && first.ts_open <= earliestAvailable) {
			historyEdgeState = 'end';
			return;
		}
		loadingOlder = true;
		olderHistoryError = null;
		historyEdgeState = 'loading';
		const barWidth = second.ts_open - first.ts_open;
		const page = previousHistoryPage({ from: first.ts_open, to: first.ts_open + barWidth * HISTORY_PAGE_BARS });
		loadBars(instrument, spec, page.from, page.to, undefined, () => !destroyed, historyLoadPriority())
			.then((resolved) => {
				if (destroyed) return;
				const older = resolved.bars.map((bar) => ({
					ts_open: bar.ts_open,
					open: bar.open / 10 ** resolved.priceScale,
					high: bar.high / 10 ** resolved.priceScale,
					low: bar.low / 10 ** resolved.priceScale,
					close: bar.close / 10 ** resolved.priceScale,
					volume: statusVolumeFromDto(bar.volume, resolved.qtyScale)
				}));
				const present = new Set(bars.map((bar) => bar.ts_open));
				const appended = older.filter((bar) => !present.has(bar.ts_open));
				if (appended.length > 0) {
					bars = [...appended, ...bars];
					// The upper bound on this pane's memory — trimmed from
					// the far side of the window the reader is actually looking
					// at, so paging into history can never itself evict the page
					// it just loaded. `retainedHistoryStart`/`MAX_HISTORY_BARS`
					// sit two orders of magnitude above the 80-bar prefetch
					// threshold above, which is the hysteresis: an ordinary
					// back-and-forth drag near one spot never approaches the cap,
					// so trimming and re-fetching the same page cannot thrash.
					if (resolved.missing.length > 0) gapRanges = [...resolved.missing, ...gapRanges];
				}
				earliestAvailable = resolved.earliestAvailable;
				if (earliestAvailable != null && (bars[0]?.ts_open ?? Infinity) <= earliestAvailable) {
					historyEdgeState = 'end';
				} else {
					historyEdgeState = 'idle';
				}
			})
			.catch((error) => {
				if (!destroyed) {
					olderHistoryError = error instanceof Error ? error.message : 'Could not load earlier history.';
					historyEdgeState = 'error';
				}
			})
			.finally(() => {
				loadingOlder = false;
			});
	}

	/** `chart.timeScale().coordinateToTime(x)` refuses to extrapolate past the
	 * last plotted bar (see that method's own doc: "cannot extrapolate time
	 * and will use the only currently existent data") — it returns `null` for
	 * any `x` in the empty right margin, which is exactly where a trend
	 * line's second point or a rectangle's corner often lands. Falls back to
	 * `coordinateToLogical` (which *does* extrapolate, since a logical index
	 * is just a continuous bar-count) and reconstructs the timestamp from the
	 * pane's own known bar spacing. */
	function xToTime(x: number): number | null {
		if (!chart) return null;
		const t = chart.timeScale().coordinateToTime(x);
		if (t != null) return Number(t);
		const logical = chart.timeScale().coordinateToLogical(x);
		if (logical == null || bars.length === 0) return null;
		const stepSeconds = TF_DURATION_SECONDS[spec as Timeframe] ?? 3600;
		const firstBarSeconds = Math.floor(bars[0].ts_open / 1_000_000_000);
		return firstBarSeconds + Math.round(logical) * stepSeconds;
	}

	/** The badge's palette, derived from the pane theme so light and dark
	 * stay in step with the rest of the chart. */
	function badgeColors(T: ReturnType<typeof themeColors>) {
		return {
			up: T.gain,
			down: T.loss,
			flat: T.neutral,
			onColor: T.bg,
			onFlat: T.onNeutral
		};
	}

	/** Reports the bar whose open is `timeSeconds`, with the close of the one
	 * before it so the header can state a change. Found by binary search:
	 * this runs on every crosshair move, over a window that can hold tens of
	 * thousands of bars. */
	function reportStatusBarAt(timeSeconds: number): void {
		if (!onStatusBar) return;
		const index = indexOfTime(renderedTimes, timeSeconds);
		if (index < 0) {
			crosshairActive = false;
			reportLatestStatusBar();
			return;
		}
		const bar = bars[index];
		if (!bar) return;
		onStatusBar({
			open: bar.open,
			high: bar.high,
			low: bar.low,
			close: bar.close,
			previousClose: index > 0 ? (bars[index - 1]?.close ?? null) : null,
			volume: bar.volume
		});
	}

	/** Whether the pointer is currently over the plot. While it is, the
	 * status line describes the bar under it and a live tick must not
	 * overwrite that with the newest bar. */
	let crosshairActive = false;

	/** Reports the newest loaded bar — what the status line shows whenever
	 * the pointer is not over the plot. Its close is the live price when one
	 * has arrived: the fetched close is a minute old the moment the bar
	 * starts forming, and a header that shows it while the badge on the same
	 * chart shows the live one is two prices for one instrument. */
	function reportLatestStatusBar(): void {
		if (!onStatusBar || crosshairActive) return;
		const index = bars.length - 1;
		const bar = bars[index];
		if (!bar) {
			onStatusBar(null);
			return;
		}
		const close = livePrice ?? bar.close;
		onStatusBar({
			open: bar.open,
			high: Math.max(bar.high, close),
			low: Math.min(bar.low, close),
			close,
			previousClose: index > 0 ? (bars[index - 1]?.close ?? null) : null,
			volume: bar.volume
		});
	}

	function themeColors(dark: boolean) {
		return chartPaneThemeColors(dark);
	}

	function isDark(): boolean {
		return isDarkTheme();
	}

	function applyTheme() {
		if (!chart || !series || !container) return;
		const T = themeColors(isDark());
		chart.applyOptions({
			layout: {
				background: { color: T.bg },
				textColor: T.axis,
				fontFamily: "'IBM Plex Mono', monospace",
				fontSize: 9,
				// Re-asserted on every theme change: `applyOptions` replaces the
				// `layout` block it is given, so omitting this here would let
				// the library's default come back. See the same option at chart
				// creation for why it is allowed to be off at all.
				attributionLogo: false,
				panes: { separatorColor: T.border, separatorHoverColor: T.cross, enableResize: true }
			},
			grid: { vertLines: { color: T.grid }, horzLines: { color: T.grid } },
			rightPriceScale: { borderColor: T.border },
			// Colours only. `rightOffset` used to be re-applied here as well, which
			// meant every theme change threw away where the reader had scrolled to
			// and snapped the chart back to the newest bar — overriding the pane's
			// own margin setting on the way past. The settings effect below owns
			// that value.
			timeScale: { borderColor: T.border },
			crosshair: {
				mode: 0,
				vertLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label },
				horzLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label }
			}
		});
		// A candle's colour can be baked into its own data point ("colour bars
		// based on previous close"), and the data effect is what writes that —
		// so a light/dark switch has to ask it for a repaint, or the previous
		// palette stays on the bars.
		themeEpoch += 1;
		series.applyOptions({
			upColor: T.up,
			downColor: T.downFill,
			borderUpColor: T.up,
			borderDownColor: T.down,
			wickUpColor: T.wickUp,
			wickDownColor: T.wickDown
		});
		priceBadge?.setColors(badgeColors(T));
	}

	onMount(() => {
		if (!container) return;
		// Warms the `GET /api/sources` cache this pane's `liveState` reads
		// from — idempotent and shared across every pane, see
		// `$lib/api/sources.svelte.ts`'s own doc.
		void ensureSourcesLoaded();
		// See `indicatorCatalog`'s own doc comment. A plain interval, not a
		// live-updating subscription: enabling/disabling a dynamic
		// indicator is an infrequent, out-of-band administrative action,
		// not something this chart needs to react to within a tick the way
		// a live price does.
		const refreshIndicatorCatalog = () => {
			fetchIndicatorCatalog()
				.then((names) => {
					if (!destroyed) indicatorCatalog = names;
				})
				.catch(() => {
					// Transient — keep whatever was already known rather than
					// treating a network blip as "every dynamic indicator was
					// just disabled".
				});
		};
		refreshIndicatorCatalog();
		const indicatorCatalogPoll = setInterval(refreshIndicatorCatalog, 15_000);
		const T = themeColors(isDark());
		chart = createChart(container, {
			layout: {
				background: { color: T.bg },
				textColor: T.axis,
				fontFamily: "'IBM Plex Mono', monospace",
				fontSize: 9,
				// Off here as well as in `applyTheme`, not only there. That
				// function runs when the theme changes, so setting it only in
				// it left the library's default (`true`) standing from mount
				// until the reader happened to switch light and dark — the
				// logo was on for the whole first visit.
				//
				// Switching it off is only allowed because the obligation it
				// exists to satisfy is met elsewhere: lightweight-charts is
				// Apache-2.0 and its licence requires naming TradingView as
				// the creator and linking to tradingview.com somewhere users
				// can reach. The About section in Settings carries both. If
				// that row is ever removed, this must go back to `true` in
				// both places — one of the two has to be there.
				attributionLogo: false,
				panes: { separatorColor: T.border, separatorHoverColor: T.cross, enableResize: true }
			},
			grid: { vertLines: { color: T.grid }, horzLines: { color: T.grid } },
			rightPriceScale: { borderColor: T.border },
			timeScale: { borderColor: T.border, rightOffset: 6 },
			crosshair: {
				mode: 0,
				vertLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label },
				horzLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label }
			},
			handleScale: { axisPressedMouseMove: true },
			width: container.clientWidth,
			height: container.clientHeight
		});
		onChartApi?.(chart);
		series = chart.addSeries(CandlestickSeries, {
			// The last-price badge draws this itself, with the countdown under
			// it in the same block; leaving this on would draw the price twice.
			lastValueVisible: false,
			upColor: T.up,
			downColor: T.downFill,
			borderUpColor: T.up,
			borderDownColor: T.down,
			wickUpColor: T.wickUp,
			wickDownColor: T.wickDown
		});
		watermark = createTextWatermark(chart.panes()[0], {
			horzAlign: 'center',
			vertAlign: 'center',
			lines: []
		});
		drawingsPrimitive = new DrawingsPrimitive();
		series.attachPrimitive(drawingsPrimitive);
		historyMarks = new HistoryMarksPrimitive();
		series.attachPrimitive(historyMarks);
		drawingsPrimitive.setOnDragEnd((id, patch) => onMoveDrawing?.(id, patch));

		// Dragging a drawing must not also pan the chart, so the chart's own
		// scroll/scale handling is switched off for the duration of a drag and
		// restored the moment it ends.
		const localPoint = (e: MouseEvent) => {
			const box = container!.getBoundingClientRect();
			return { x: e.clientX - box.left, y: e.clientY - box.top };
		};
		onPointerDown = (e: MouseEvent) => {
			if (e.button !== 0 || tool !== 'cursor' || !drawingsPrimitive) return;
			const at = localPoint(e);
			const found = drawingAt(drawingsPrimitive, at.x, at.y);
			if (!found) {
				onSelectDrawing?.(null);
				return;
			}
			onSelectDrawing?.(found.id);
			drawingsPrimitive.beginDrag(found.id, found.hit, at.x, at.y);
			chart?.applyOptions({
				handleScroll: false,
				handleScale: false
			});
		};
		onPointerMove = (e: MouseEvent) => {
			if (!drawingsPrimitive?.isDragging()) return;
			const at = localPoint(e);
			drawingsPrimitive.updateDrag(at.x, at.y);
		};
		onPointerUp = () => {
			if (drawingsPrimitive?.isDragging()) {
				drawingsPrimitive.endDrag();
				chart?.applyOptions({ handleScroll: true, handleScale: true });
				return;
			}
			// Dragging the price axis switches `autoScale` off inside the
			// library without telling anyone. Read it back after every release
			// so the stored setting follows the chart rather than contradicting
			// it.
			if (!series) return;
			const live = series.priceScale().options().autoScale;
			if (live !== undefined && live !== paneSettings.autoScale) {
				onAutoScaleChanged?.(live);
			}
		};
		container.addEventListener('mousedown', onPointerDown);
		window.addEventListener('mousemove', onPointerMove);
		window.addEventListener('mouseup', onPointerUp);
		// Also on `pointerup`: the chart library handles the price axis with
		// its own listeners, and which of the two event families a given
		// release surfaces as is not ours to assume. Both paths run the same
		// idempotent read-back.
		window.addEventListener('pointerup', onPointerUp);
		priceBadge = new LastPriceBadgePrimitive();
		series.attachPrimitive(priceBadge);
		priceBadge.setColors(badgeColors(T));

		chart.subscribeCrosshairMove((param) => {
			// The bar under the pointer, reported as numbers for the header to
			// format. Off the plot, the header falls back to the newest bar
			// (`reportLatestStatusBar`) rather than emptying — a status line
			// that blanks whenever the pointer leaves is a line nobody can
			// read at rest.
			if (param.point && param.time !== undefined) {
				crosshairActive = true;
				reportStatusBarAt(Number(param.time));
			} else {
				crosshairActive = false;
				reportLatestStatusBar();
			}
			// Rubber-band preview of a two-anchor tool's second point while the
			// first is already placed — every tool in `TOOL_ANCHOR_COUNT` with
			// `anchors === 2` gets this line, not just trend/rectangle.
			if (pendingStart && chart && series && param.point) {
				const price = series.coordinateToPrice(param.point.y);
				const time = xToTime(param.point.x);
				if (price != null && time != null) {
					drawingsPrimitive?.setPending(pendingStart, { time, price });
					// A two-anchor tool with no `DrawingKindTag` (measure, today)
					// never persists — see the click handler below — so its
					// preview is a live delta readout, not only a rubber-band line.
					if (!DRAWING_KIND_FOR_TOOL[tool] && TOOL_ANCHOR_COUNT[tool] === 2) {
						const step = TF_DURATION_SECONDS[spec as Timeframe] ?? 3600;
						const stats = measureStats({ time: pendingStart.time, value: pendingStart.price }, { time, value: price }, step);
						drawingsPrimitive?.setMeasurePreview({ time, price }, formatMeasureLabel(stats));
					}
				}
			}
		});

		chart.timeScale().subscribeVisibleLogicalRangeChange((range) => {
			// The chart has processed a range, so nothing is pending any more —
			// whether that was the frame this pane asked for or the reader's own
			// pan, which supersedes it. Leaving it set would pin the viewport and
			// the reader could never move again.
			pendingFrame = null;
			if (shouldPrefetchHistory(range?.from == null ? null : Number(range.from), bars.length)) {
				requestOlderHistory();
			}
		});

		chart.subscribeClick((param) => {
			if (!param.point || !chart || !series) return;

			// The cursor tool selects on mousedown (below), not here: two
			// deciders meant the click following a mousedown could clear what
			// that mousedown had just selected, so a drawing only stayed
			// selected while the button was held.
			if (tool === 'cursor') return;

			// How many clicks this tool needs, and — for the tools that
			// persist one — which `DrawingKindTag` it becomes. Both come from
			// one data table (`$lib/charts/pane-runtime.ts`): a tool this
			// pane does not know how to act on (no entry in `TOOL_ANCHOR_COUNT`)
			// is inert here rather than needing a branch of its own.
			const anchors = TOOL_ANCHOR_COUNT[tool];
			if (!anchors) return;
			const price = series.coordinateToPrice(param.point.y);
			const time = xToTime(param.point.x);
			if (price == null || time == null) return;
			const point = { time, price };

			if (anchors === 1) {
				const kind = DRAWING_KIND_FOR_TOOL[tool];
				if (kind) {
					// One payload for every single-click tool: `price` is all a
					// `horizontal_line` reads, `start`/`text`/`anchor` are all a
					// `text_note` reads (see `drawingToInput`) — no per-tool branch
					// needed for the one field each actually uses.
					onCreateDrawing?.({
						kind,
						price: point.price,
						start: point,
						text: DEFAULT_TEXT_NOTE_TEXT,
						anchor: DEFAULT_TEXT_NOTE_ANCHOR,
						// Recorded at the moment it is drawn, by the pane that knows
						// what it was drawn on. Reading it back off the pane later
						// would be reading whatever the pane has been switched to
						// since, which is a different fact.
						instrument,
						...DEFAULT_DRAWING_STYLE
					});
				}
				onToolConsumed?.();
				return;
			}

			// Every two-anchor tool: first click sets the start, second
			// finishes it.
			if (!pendingStart) {
				pendingStart = point;
				drawingsPrimitive?.setPending(point, point);
				return;
			}
			const kind = DRAWING_KIND_FOR_TOOL[tool];
			if (kind) {
				onCreateDrawing?.({ kind, start: pendingStart, end: point, instrument, ...DEFAULT_DRAWING_STYLE });
				pendingStart = null;
				drawingsPrimitive?.setPending(null, null);
				onToolConsumed?.();
				return;
			}
			// No persisted kind for this tool (measure, today): a transient
			// reading, never written back through `onCreateDrawing` — see
			// `$lib/charts/measure.ts`. The tool is deliberately *not*
			// consumed here: unlike a drawing, a measurement is expected to
			// be taken again without reselecting the tool each time, and
			// consuming it would also erase this very readout (the
			// tool-change effect below clears it as soon as `tool` stops
			// being `'measure'`).
			const step = TF_DURATION_SECONDS[spec as Timeframe] ?? 3600;
			const stats = measureStats({ time: pendingStart.time, value: pendingStart.price }, { time: point.time, value: point.price }, step);
			drawingsPrimitive?.setMeasurePreview(point, formatMeasureLabel(stats));
			pendingStart = null;
			drawingsPrimitive?.setPending(null, null);
		});

		ro = new ResizeObserver(() => {
			if (!container || !chart) return;
			chart.applyOptions({ width: container.clientWidth, height: container.clientHeight });
			const narrow = container.clientWidth < 330;
			if (narrow !== lastNarrow) {
				lastNarrow = narrow;
				onNarrow?.(narrow);
			}
			reportPaneTops();
		});
		ro.observe(container);

		paneRo = new ResizeObserver(() => {
			if (!destroyed) reportPaneTops();
		});

		mo = new MutationObserver(applyTheme);
		mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });

		return () => {
			destroyed = true;
			clearInterval(indicatorCatalogPoll);
			ro?.disconnect();
			paneRo?.disconnect();
			mo?.disconnect();
			overlaySeries.clear();
			overlayInstrumentSeries.clear();
			if (onPointerDown) container?.removeEventListener('mousedown', onPointerDown);
			if (onPointerMove) window.removeEventListener('mousemove', onPointerMove);
			if (onPointerUp) window.removeEventListener('mouseup', onPointerUp);
			if (onPointerUp) window.removeEventListener('pointerup', onPointerUp);
			onPointerDown = onPointerMove = onPointerUp = undefined;
			drawingsPrimitive = undefined;
			// Belt-and-braces alongside whatever `series`'s own disposal already
			// calls: stops the primitive's own one-second timer even if the
			// library's primitive-detach timing ever changes.
			priceBadge?.detached();
			priceBadge = undefined;
			// Fired before `chart.remove()` so a caller unsubscribing from this
			// native panes share the chart's time scale and never touch it
			// after it is gone.
			onChartApi?.(undefined);
			try {
				chart?.remove();
			} catch {
				// already gone
			}
		};
	});

	// A new instrument/timeframe (or the eraser button bumping `clearToken`,
	// which persists an empty drawing list — see `clearPaneDrawings`) means a
	// trend line/rectangle mid-draw no longer applies. Kept separate from the
	// data effect below so replay ticks (which change `replayIdx` on the same
	// instrument) don't cancel an in-progress drawing every 220ms.
	$effect(() => {
		void instrument;
		void spec;
		void clearToken;
		pendingStart = null;
		drawingsPrimitive?.setPending(null, null);
	});

	// The measure tool's readout is never persisted (`$lib/charts/measure.ts`)
	// and is deliberately not cleared by finishing a measurement (see the
	// click handler above) — it only goes away once the reader actually
	// switches to a different tool, which is what this effect watches for.
	$effect(() => {
		if (tool !== 'measure') {
			drawingsPrimitive?.setMeasurePreview(null, null);
		}
	});

	// Native panes share the chart's time scale and crosshair. Indicator
	// warm-up can therefore shorten a plot without changing which candle is
	// under its last point; there is no second logical index to synchronize.
	$effect(() => {
		if (!chart || bars.length === 0) return;
		const range = loadedRange;
		if (!range) return;
		const activeChart = chart;
		const currentInstrument = instrument;
		const currentSpec = spec;
		const currentPriceScale = priceScale;
		// See the overlay-indicator effect's own comment on `indicatorCatalog`
		// for why a layer outside it is excluded here rather than plotted.
		const catalog = indicatorCatalog;
		const layers = subPaneLayers.filter(
			(layer) => layer.indicatorName && catalog.has(layer.indicatorName)
		);
		const token = subPaneGeneration.begin();
		const { from, to } = range;

		Promise.all(
			layers.map(async (layer, paneIndex) => ({
				layer,
				paneIndex: paneIndex + 1,
				byField: (
					await loadIndicatorSeries(
						currentInstrument,
						currentSpec,
						from,
						to,
						layer.indicatorName as string,
						layer.params
					)
				).byField
			}))
		)
			.then((results) => {
				if (destroyed || !subPaneGeneration.isCurrent(token)) return;
				const seen = new Set<string>();
				for (const { layer, paneIndex, byField } of results) {
					for (const plot of plotsForLayer(layer.id, layer.indicatorName as string)) {
						const points = byField.get(plot.field) ?? [];
						if (points.length === 0) continue;
						const key = `${layer.id}:${plot.field}`;
						seen.add(key);
						let handle = subPaneSeries.get(key);
						if (!handle) {
							handle =
								plot.type === 'histogram'
									? activeChart.addSeries(HistogramSeries, { priceLineVisible: false }, paneIndex)
									: activeChart.addSeries(LineSeries, { priceLineVisible: false }, paneIndex);
							subPaneSeries.set(key, handle);
						} else {
							handle.moveToPane(paneIndex);
						}
						const divisor =
							indicatorFieldScale(layer.indicatorName as string) === 'price'
								? 10 ** currentPriceScale
								: 1;
						handle.applyOptions(
							plot.type === 'line'
								? {
										color: plot.color,
										lineWidth: plot.width as 1 | 2 | 3 | 4,
										lineStyle: LINE_STYLE_MAP[plot.style],
										visible: layer.visible && plot.visible
									}
								: { color: plot.color, visible: layer.visible && plot.visible }
						);
						holdViewport(() => {
							handle.setData(nativePaneData(points, divisor));
						});
					}
				}
				for (const [key, handle] of subPaneSeries) {
					if (seen.has(key)) continue;
					activeChart.removeSeries(handle);
					subPaneSeries.delete(key);
				}
				// Removing the last series in a pane does not remove the pane.
				// A pane left behind keeps its slice of the height and its own
				// price scale, so the panes below it are pushed down and no
				// longer line up with the headers that label them — and the
				// empty scale draws a range belonging to nothing. Walk from the
				// back so removing one never renumbers a pane still to check,
				// and never touch pane 0, which holds the candles.
				for (let index = activeChart.panes().length - 1; index >= 1; index -= 1) {
					if (activeChart.panes()[index]?.getSeries().length === 0) {
						activeChart.removePane(index);
					}
				}
				// Stretch factors, not absolute pixels. A pixel height computed
				// from `container.clientHeight` is only correct at the instant
				// this effect runs: the ResizeObserver below resizes the chart
				// but cannot know these fractions, so after any resize the
				// panes keep heights measured against a container that no
				// longer exists — and the percent-positioned pane headers,
				// which do follow the container, detach from the panes they
				// label. A stretch factor is a relative weight the chart
				// re-divides on every resize by itself, so both stay in step
				// with no resize handler at all.
				// Only when the set of panes actually changed. This effect re-runs on
				// every layout update — showing or hiding a layer is one — and
				// rewriting the stretch factors each time overwrites a separator the
				// reader has dragged. Hiding an indicator should make the indicator
				// disappear and change nothing else about the pane.
				const panes = activeChart.panes();
				const weights = paneStretchFactors(panes.length, layers.length, settings?.paneSplit);
				const weightKey = weights.join(',');
				if (weightKey !== appliedPaneWeights) {
					appliedPaneWeights = weightKey;
					for (const [index, pane] of panes.entries()) {
						pane.setStretchFactor(weights[index] ?? 1);
					}
				}
				reportPaneTops();
			})
			.catch(() => {
				// A failed indicator fetch leaves candles and other panes usable.
			});
	});

	// The two location-bound statements go to the chart, not to a notice: the
	// earliest bar the venue has, and every range the server reported missing.
	// Both are facts about a place on the time axis, so they are drawn there
	// and pan with the bars.
	$effect(() => {
		if (!historyMarks) return;
		const stepSeconds = TF_DURATION_SECONDS[spec as Timeframe] ?? 3600;
		const first = bars[0];
		historyMarks.setGrid(
			first ? Math.floor(first.ts_open / 1_000_000_000) : undefined,
			stepSeconds
		);
		historyMarks.setStartOfHistory(
			historyEdgeState === 'end' && first ? Math.floor(first.ts_open / 1_000_000_000) : null
		);
		historyMarks.setGaps(gapRanges.map((gap) => ({ from: gap.from, to: gap.to })));
	});

	// `timeVisible` defaults to `false` (dates only — every intraday tick
	// used to repeat the same day-of-month with no time on it at all) and
	// `secondsVisible` defaults to `true` (which would print `14:15:00` on
	// every spec coarser than a second the moment `timeVisible` turns on).
	// Both are driven from the pane's current spec and re-applied here
	// whenever it changes, not only at chart creation — switching timeframe
	// must relabel the axis without remounting the chart.
	$effect(() => {
		if (!chart) return;
		const intraday = isIntradaySpec(spec);
		// Guarded like every other scale write here: applying an option the
		// scale already holds costs the user their scroll position.
		if (chart.timeScale().options().timeVisible !== intraday) {
			chart.applyOptions({ timeScale: { timeVisible: intraday, secondsVisible: false } });
		}
	});

	// Keeps the primitive's drawing list and selection in sync with this
	// pane's real, persisted drawings.
	$effect(() => {
		drawingsPrimitive?.setDrawings(drawings);
	});
	$effect(() => {
		drawingsPrimitive?.setSelected(selectedDrawingId);
	});
	// The primitive's own bar grid — see `DrawingsPrimitive.setBarGrid`'s doc
	// on why it needs one at all.
	$effect(() => {
		const currentSpec = spec;
		if (bars.length === 0) return;
		const firstBarSeconds = Math.floor(bars[0].ts_open / 1_000_000_000);
		const stepSeconds = TF_DURATION_SECONDS[currentSpec as Timeframe] ?? 3600;
		drawingsPrimitive?.setBarGrid(firstBarSeconds, stepSeconds);
	});

	// Loads bars whenever the instrument/timeframe changes (`loadBars` itself is cache-first — the second open of the same range issues zero venue requests).
	$effect(() => {
		const currentInstrument = instrument;
		const currentSpec = spec;
		// Read so a closing bar re-runs this: the tick fold draws a candle
		// assembled from whatever ticks this client saw, and the bar the
		// venue actually recorded is the one that should replace it.
		void barEpoch;
		// The manual reload: same path as a bar closing, so a chart that has
		// gone wrong is rebuilt from the loader rather than nudged.
		void reloadToken;
		const token = loadGeneration.begin();
		const identity = `${currentInstrument}|${currentSpec}`;
		if (loadedIdentity !== identity) {
			loadedIdentity = identity;
			// The previous selection's candles must not survive onto a new
			// instrument or timeframe — cleared immediately, not once the new
			// load resolves or fails, so nothing pretends to be current while
			// it waits. `progressLabel`'s loading state covers the gap.
			//
			// Only on a real change of series, though: a bar closing re-runs
			// this too, and blanking the chart once a minute would be worse
			// than the stale candle this exists to prevent.
			bars = [];
			nextBarOpenAt = null;
			earliestAvailable = null;
			historyEdgeState = 'idle';
			gapRanges = [];
			liveCandle = null;
		}
		// The pane's own window, not the initial one. This effect re-runs on every
		// bar close, and re-requesting the opening window discards every page of
		// history the reader scrolled in.
		const { from, to } = reloadWindow(bars, initialBarWindow(currentSpec));
		loadBars(
			currentInstrument,
			currentSpec,
			from,
			to,
			(p) => {
				if (!destroyed && loadGeneration.isCurrent(token)) progress = p;
				if (p.phase === 'error') {
					// The raw transport message (URL, query string, status
					// code) belongs in the console, never on screen —
					// `progressLabel` shows product copy instead.
					console.error(`Chart pane ${currentInstrument} ${currentSpec}: ${p.message}`);
				}
			},
			() => !destroyed && loadGeneration.isCurrent(token)
		)
			.then((resolved) => {
				if (destroyed || !loadGeneration.isCurrent(token)) return;
				priceScale = resolved.priceScale;
				qtyScale = resolved.qtyScale;
				onPriceScale?.(resolved.priceScale);
				nextBarOpenAt = resolved.nextBarOpenAt;
				earliestAvailable = resolved.earliestAvailable;
				gapRanges = resolved.missing;
				bars = resolved.bars.map((b) => ({
					ts_open: b.ts_open,
					open: b.open / 10 ** resolved.priceScale,
					high: b.high / 10 ** resolved.priceScale,
					low: b.low / 10 ** resolved.priceScale,
					close: b.close / 10 ** resolved.priceScale,
					volume: statusVolumeFromDto(b.volume, resolved.qtyScale)
				}));
				if (bars.length > 0) onLastClose?.(bars[bars.length - 1].close);
			})
			.catch(() => {
				// `progress` already carries the error phase/message; nothing
				// further to do here.
			});
	});

	// A "go to date" request. Unlike paging left (which extends the loaded
	// range), a jump can land anywhere and must not fold onto what was
	// there before — two time-disconnected ranges drawn as one series is
	// two time-disconnected ranges drawn as one series, so `applyHistoryJump` below
	// replaces the window rather than merging into it.
	//
	// `jumpTarget` is the only thing this effect may track, same reasoning
	// as `resetToken` above: everything it reads to actually perform the
	// jump is read inside `untrack` so a plain instrument/timeframe change
	// does not replay the last jump target against the new series.
	$effect(() => {
		const request = jumpTarget;
		if (!request || request.token === 0) return;
		untrack(() => {
			const currentInstrument = instrument;
			const currentSpec = spec;
			const token = loadGeneration.begin();
			const stepSeconds = TF_DURATION_SECONDS[currentSpec as Timeframe] ?? 3600;
			const barWidth = stepSeconds * 1_000_000_000;
			const nowNanos = Date.now() * 1_000_000;
			const window = resolveJumpWindow(request.targetNanos, barWidth, nowNanos, earliestAvailable);
			progress = { phase: 'checking' };
			loadBars(
				currentInstrument,
				currentSpec,
				window.from,
				window.to,
				(p) => {
					if (!destroyed && loadGeneration.isCurrent(token)) progress = p;
				},
				() => !destroyed && loadGeneration.isCurrent(token)
			)
				.then((resolved) => {
					if (destroyed || !loadGeneration.isCurrent(token)) return;
					priceScale = resolved.priceScale;
					qtyScale = resolved.qtyScale;
					onPriceScale?.(resolved.priceScale);
					nextBarOpenAt = resolved.nextBarOpenAt;
					earliestAvailable = resolved.earliestAvailable;
					gapRanges = resolved.missing;
					const jumped = resolved.bars.map((b) => ({
						ts_open: b.ts_open,
						open: b.open / 10 ** resolved.priceScale,
						high: b.high / 10 ** resolved.priceScale,
						low: b.low / 10 ** resolved.priceScale,
						close: b.close / 10 ** resolved.priceScale,
						volume: statusVolumeFromDto(b.volume, resolved.qtyScale)
					}));
					bars = applyHistoryJump(bars, jumped);
					// A landing before the venue's known left edge is still
					// honestly "the end of history", the same statement panning
					// there produces — the reader should not have to pan left
					// once more just to be told what the jump already found.
					historyEdgeState =
						earliestAvailable != null && (bars[0]?.ts_open ?? Infinity) <= earliestAvailable
							? 'end'
							: 'idle';
					if (bars.length > 0) onLastClose?.(bars[bars.length - 1].close);
					// A jump is a deliberate change of viewport — unlike paging
					// left (which preserves it), the reader asked to look
					// somewhere else, so the frame should actually move there.
					// Cleared rather than framed here: the bars have only just
					// been assigned, and the data effect frames the window it
					// actually renders, one step later.
					framedFor = '';
				})
				.catch(() => {
					// `progress` already carries the error phase/message.
				});
		});
	});

	/** `'auto'`/`'THEME'` mean "use the palette", so a user who never opened
	 * the settings dialog still gets a coherent chart, and one who set a
	 * colour and then cleared it goes back to the theme rather than to
	 * whatever the value happened to be. */
	function pick(value: string | undefined, fallback: string): string {
		return !value || value === 'auto' || value === 'THEME' ? fallback : value;
	}

	const paneSettings = $derived(settings ?? defaultChartSettings());

	// Everything the settings dialog promises about how the chart looks is
	// applied here. Without this the dialog saved and reloaded correctly and
	// changed nothing on screen.
	$effect(() => {
		if (!chart || !series) return;
		const T = themeColors(isDark());
		const cs = paneSettings;

		series.applyOptions({
			upColor: pick(cs.bodyUp, T.up),
			downColor: pick(cs.bodyDown, T.downFill),
			borderVisible: cs.borders,
			borderUpColor: pick(cs.borderUp, T.up),
			borderDownColor: pick(cs.borderDown, T.down),
			wickVisible: cs.wicks,
			wickUpColor: pick(cs.wickUp, T.wickUp),
			wickDownColor: pick(cs.wickDown, T.wickDown),
			priceFormat:
				cs.precision === 'DEFAULT'
					? { type: 'price', precision: priceScale, minMove: 10 ** -priceScale }
					: {
							type: 'price',
							precision: Number(cs.precision),
							minMove: 10 ** -Number(cs.precision)
						}
		});

		chart.applyOptions({
			layout: {
				background: { color: cs.bgMode === 'SOLID' ? cs.bgColor : T.bg },
				textColor: pick(cs.scaleTextColor, T.axis),
				fontSize: cs.scaleFont
			},
			grid: {
				vertLines: { visible: cs.gridV, color: pick(cs.gridVColor, T.grid) },
				horzLines: { visible: cs.gridH, color: pick(cs.gridHColor, T.grid) }
			},
			crosshair: {
				vertLine: {
					style: LINE_STYLE_MAP[cs.crosshair] ?? 2,
					color: pick(cs.crosshairColor, T.cross)
				},
				horzLine: {
					style: LINE_STYLE_MAP[cs.crosshair] ?? 2,
					color: pick(cs.crosshairColor, T.cross)
				}
			},
			rightPriceScale: { borderColor: pick(cs.scaleLineColor, T.border) },
			timeScale: { visible: cs.timeAxis, borderColor: pick(cs.scaleLineColor, T.border) }
		});

		// Same reasoning: re-writing identical margins would reset the range.
		const margins = series.priceScale().options().scaleMargins;
		const wantTop = cs.marginTop / 100;
		const wantBottom = cs.marginBottom / 100;
		if (margins?.top !== wantTop || margins?.bottom !== wantBottom) {
			series.priceScale().applyOptions({ scaleMargins: { top: wantTop, bottom: wantBottom } });
		}
		// Guarded for the same reason as the price scale above: re-writing
		// `rightOffset` when it is already correct throws away where the user
		// has scrolled to, which is why panning left and then touching the
		// price axis snapped the chart back to the newest bar.
		if (chart.timeScale().options().rightOffset !== cs.marginRight) {
			chart.timeScale().applyOptions({ rightOffset: cs.marginRight });
		}

		// How the price axis reads: linear, logarithmic, or one of the two
		// relative modes, and whether it refits itself. `autoScale` is a
		// stored setting rather than a transient because dragging the axis
		// turns it off and it stays off — a chart can otherwise be left in a
		// state no amount of panning recovers.
		const MODES = { REGULAR: 0, LOGARITHMIC: 1, PERCENT: 2, INDEXED: 3 } as const;
		// Only the keys that actually differ. Writing a price-scale option
		// that is already correct is not free: `applyOptions` discards the
		// manual range, so re-applying `autoScale: false` right after the user
		// dragged the axis threw away the very view the drag produced — the
		// setting followed the chart, and then the chart snapped back.
		const live = series.priceScale().options();
		const wanted = {
			mode: MODES[cs.priceScaleMode] ?? 0,
			invertScale: cs.invertScale,
			autoScale: cs.autoScale
		};
		const changed = Object.fromEntries(
			Object.entries(wanted).filter(([key, value]) => live[key as keyof typeof live] !== value)
		);
		if (Object.keys(changed).length > 0) series.priceScale().applyOptions(changed);

		// Which side the price scale lives on.
		chart.applyOptions({
			rightPriceScale: { visible: cs.placement === 'RIGHT' || cs.placement === 'BOTH' },
			leftPriceScale: { visible: cs.placement === 'LEFT' || cs.placement === 'BOTH' }
		});

		// `VALUE + LINE` shows both the label and the line across the plot;
		// `VALUE` drops the line; `HIDDEN` drops both. The label itself is the
		// badge this pane draws, so it is switched there.
		series.applyOptions({ priceLineVisible: cs.symbolLabel === 'VALUE + LINE' });

		watermark?.applyOptions({
			visible: cs.watermark === 'VISIBLE',
			lines: [{
				text: instrument.toUpperCase(),
				color: pick(cs.watermarkColor, T.grid),
				fontSize: 44
			}]
		});
	});

	// Time-axis labels and the crosshair's own time label: 24h or 12h, with
	// or without the weekday, and always in the viewer's chosen display zone
	// (`userZoneStore.zone`) rather than the browser's. The library formats
	// both itself unless given formatters, and its defaults are the 24h axis
	// with no weekday and a crosshair label that ignores this app's zone
	// entirely.
	/** What `buildTimeLabelOptions` was last built for, `spec`/`dayOfWeek`/
	 * `timeFormat`/zone folded into one comparable string
	 * (`timeLabelOptionsSignature`) since a formatter closure's identity
	 * always differs and is nothing to compare against. `buildTimeLabelOptions`'s
	 * return type only ever carries the two label-formatting fields
	 * (`TimeLabelOptions`), so re-running this on a zone change cannot reach
	 * for `timeScale.rightOffset` or a visible-range field the way an editor
	 * of this block might otherwise be tempted to — writing either of those
	 * here would discard where the reader has scrolled to, exactly the
	 * `applyTheme` history this file's own comments already warn about. */
	let tickFormatSignature = '';

	$effect(() => {
		if (!chart) return;
		const cs = paneSettings;
		const zoneId = userZoneStore.zone;
		const signature = timeLabelOptionsSignature(spec, cs.dayOfWeek, cs.timeFormat, zoneId);
		if (signature === tickFormatSignature) return;
		tickFormatSignature = signature;
		chart.applyOptions(buildTimeLabelOptions(spec, cs.dayOfWeek, cs.timeFormat, zoneId));
	});

	/** What the overlay set actually is, as a value rather than an array
	 * identity. The identity changes on every parent render — and the parent
	 * re-renders on every live tick — so an effect keyed on it recomputed
	 * every indicator several times a second, which is both the spinner that
	 * never stopped and the freeze behind it. */
	const overlaySignature = $derived(
		overlayLayers
			.map((l) => `${l.id}:${l.kind}:${l.indicatorName ?? ''}:${JSON.stringify(l.params)}:${l.visible}`)
			.join('|')
	);

	/** Same reasoning as `overlaySignature` above, for `overlay_instrument`
	 * layers — `$lib/charts/overlay-instrument.ts`'s own value, so the
	 * decision of what counts as "changed" lives in one tested place instead
	 * of being re-derived here by hand. */
	const instrumentOverlaySignature = $derived(overlayInstrumentSignature(overlayLayers));

	/** The last set reported upward. Reporting is a write into the parent,
	 * and the parent re-render hands this effect a fresh `overlayLayers`
	 * array — so an unconditional report re-runs the effect that made it, and
	 * the two chase each other until Svelte gives up. Reporting only real
	 * changes breaks that, and `untrack` keeps the report itself out of the
	 * effect's dependencies. */
	let reportedLoading = '';
	/** The instrument, timeframe, window and overlay set the indicators are
	 * currently drawn for. Anything else changing is a tick, not a load. */
	let lastLoadedOverlays = '';
	function reportLoading(ids: string[]) {
		const signature = ids.join(',');
		if (signature === reportedLoading) return;
		reportedLoading = signature;
		untrack(() => onLayerLoading?.(ids));
	}

	/** The high/low guides, kept so they can be removed when the setting is
	 * turned off or the data changes. */
	let highLowLines: ReturnType<NonNullable<typeof series>['createPriceLine']>[] = [];
	/** The two independently-owned best-bid-and-offer guides. */
	let quoteLines: ReturnType<NonNullable<typeof series>['createPriceLine']>[] = [];

	// High and low lines: horizontal guides at the extremes of what is
	// loaded, which is what makes a range readable at a glance.
	$effect(() => {
		if (!series) return;
		const showing = paneSettings.highLow;
		const data = bars;
		for (const line of highLowLines) {
			try {
				series.removePriceLine(line);
			} catch {
				// the series was rebuilt underneath us; nothing to remove
			}
		}
		highLowLines = [];
		if (!showing || data.length === 0) return;
		const T = themeColors(isDark());
		const high = Math.max(...data.map((b) => b.high));
		const low = Math.min(...data.map((b) => b.low));
		highLowLines = [
			series.createPriceLine({
				price: high,
				color: T.gain,
				lineWidth: 1,
				lineStyle: 2,
				axisLabelVisible: true,
				title: 'H'
			}),
			series.createPriceLine({
				price: low,
				color: T.loss,
				lineWidth: 1,
				lineStyle: 2,
				axisLabelVisible: true,
				title: 'L'
			})
		];
	});

	// Quote lines are intentionally a separate lease from last-trade ticks.
	// A venue can offer either capability, and the UI must never infer one
	// from the other. Waiting for the sources response also avoids asking the
	// socket for an unsupported stream while capabilities are still unknown.
	$effect(() => {
		const quoteTopic = `quote:${instrument}`;
		if (hasQuoteFeed(instrument) !== true) {
			liveQuote = null;
			return;
		}
		wsClient.subscribe(quoteTopic);
		liveQuote = null;
		return () => wsClient.unsubscribe(quoteTopic);
	});

	/** The chart library draws its price and time scales inside the same
	 * canvas as the plot, so a right-click's region can only be told apart by
	 * how close the pointer is to the edge. These match the widths the
	 * library lays out at this font size; they are read only to route a
	 * context menu, so being a few pixels out costs nothing. */
	const PRICE_SCALE_WIDTH_PX = 64;
	const TIME_SCALE_HEIGHT_PX = 24;

	/** Returns the pane to its default frame: the loaded range, price axis
	 * auto-scaling again. Dragging the price axis turns `autoScale` off and
	 * it stays off, so a chart can be left in a state no amount of panning
	 * recovers — this is the way back. */
	export function resetView(): void {
		if (!chart || !series) return;
		series.priceScale().applyOptions({ autoScale: true });
		// The margin to the right of the newest bar is part of the default view,
		// and a chart the reader has dragged can be sitting anywhere relative to
		// it — so it is re-applied here, not only framed below, which is what
		// keeps it once new bars start auto-scrolling the chart again.
		const offset = paneSettings.marginRight;
		if (chart.timeScale().options().rightOffset !== offset) {
			chart.timeScale().applyOptions({ rightOffset: offset });
		}
		frameDefaultView();
		// Reported, not just applied: the axis shortcut and the menu's tick
		// both read the stored setting, and a reset that only moved the chart
		// would leave them describing the state it was in before.
		onAutoScaleChanged?.(true);
	}

	// Reset on request. `0` is the never-asked value, so a pane that has not
	// been asked is left exactly as the user arranged it.
	//
	// `resetToken` is the only thing this effect may track. `resetView`
	// reports the new auto-fit state upward, that report comes back down as a
	// prop, and an effect that tracked its own body's reads would re-run on
	// its own report — which is exactly the update-depth loop this guard
	// exists to stop.
	$effect(() => {
		const token = resetToken;
		if (token === 0) return;
		untrack(() => resetView());
	});

	/** The series the viewport was last framed for. Panning or zooming is the
	 * user's own choice and must survive a live tick or a replay step, so the
	 * frame is only reset when the pane starts showing a *different* series. */
	let framedFor = '';
	/** The range `frameDefaultView` last asked the chart for and the chart has
	 * not reported back yet. A series update landing in this window would
	 * otherwise read the range as it was before the request and write it back,
	 * silently discarding the frame — which only ever happened when a second
	 * series existed to do the reading, and is why a bare chart framed
	 * correctly while the same chart with one indicator did not. */
	let pendingFrame: LogicalRange | null = null;
	let pendingFrameAt = 0;

	/** The pending frame, or `null` once it has expired. Read through this and
	 * never directly: a frame the chart silently declined is never reported and
	 * so never cleared, and one that outlives its race pins the viewport for the
	 * rest of the session. */
	function livePendingFrame(): LogicalRange | null {
		if (pendingFrame && !pendingFrameIsLive(pendingFrameAt, Date.now())) pendingFrame = null;
		return pendingFrame;
	}

	/** Every structural series update goes through here, so none of them can
	 * forget the pending frame. */
	function holdViewport(update: () => void, shift: number | null = 0): void {
		if (!chart) return;
		preserveVisibleRange(chart.timeScale(), update, shift, livePendingFrame());
	}

	/** Length plus both ends identifies a contiguous bar window, so the
	 * timeline is only published when it really moved — a colour setting
	 * re-running the effect below must not make every sub-pane re-draw. */
	let publishedBarTimes = '';

	/** The bar times currently handed to the series, in ascending order. Read
	 * back on the next update to work out how far the bars moved
	 * (`logicalRangeShift`) and how many there are to frame. */
	let renderedTimes: number[] = [];
	/** Everything the drawn candles depend on. `setData` is not free — it
	 * re-lays out the whole series — and this effect re-runs whenever the
	 * pane's settings object is *replaced*, which happens on every layout
	 * reload even when not one value inside it differs. */
	let renderedDataKey = '';
	/** The replay position the series was last drawn at, or `null` outside
	 * replay. Replay reveals bars by moving the window's *right* edge, which
	 * the ordinary "hold the reader's frame still" rule would leave off
	 * screen. */
	let renderedCut: number | null = null;

	/** Frames the pane on its default view: the most recent bars, with the
	 * pane's own margin still clear to the right of the newest one.
	 *
	 * `timeScale().width()` is the plot's own width — the container minus the
	 * price axis — so the bar width this computes fills exactly the space
	 * bars are drawn in. Before the chart has been laid out it reports `0`,
	 * and `defaultFrame` returns nothing rather than dividing by it. */
	function frameDefaultView(): void {
		if (!chart) return;
		const timeScale = chart.timeScale();
		// Counted from the bars this pane holds right now, not from
		// `renderedTimes` — that cache is only written inside the data effect's
		// own guard, so anything framing before it runs (the reset button, a
		// freshly loaded window) reads the *previous* series' length. Getting it
		// too small is not a small error: `barSpacing` is the plot width divided
		// by it, so a stale count of a handful leaves one candle filling the
		// pane, and the price axis then auto-fits to that single bar.
		const drawn =
			replayIdx == null ? bars.length : Math.max(30, Math.min(bars.length, replayIdx));
		const frame = defaultFrame(drawn, paneSettings.marginRight);
		if (!frame) return;
		// Only pending when the chart actually has a change to make. Marking an
		// unchanged frame pending leaves it pending forever, because the chart
		// never reports a range it did not apply.
		pendingFrame = frameIsPending(timeScale.getVisibleLogicalRange(), frame) ? frame : null;
		pendingFrameAt = Date.now();
		timeScale.setVisibleLogicalRange(frame);
	}

	// Re-set data whenever the resolved bars or the replay position change.
	$effect(() => {
		if (!series || !chart) return;
		const identity = `${instrument}|${spec}`;
		const cut = replayIdx == null ? bars.length : Math.max(30, Math.min(bars.length, replayIdx));
		const T = themeColors(isDark());
		const cs = paneSettings;
		const up = pick(cs.bodyUp, T.up);
		const down = pick(cs.bodyDown, T.downFill);
		const window = bars.slice(0, cut);
		const barTimes = window.map((b) => Math.floor(b.ts_open / 1_000_000_000) as UTCTimestamp);
		const timesKey = `${barTimes.length}|${barTimes[0] ?? ''}|${barTimes[barTimes.length - 1] ?? ''}`;
		// Everything a drawn candle depends on. Without this guard the series
		// is re-laid-out on every settings *replacement* — and the pane's
		// settings object is replaced wholesale on every layout reload, so
		// creating a drawing or dragging the price axis re-set three hundred
		// candles that had not changed at all.
		const dataKey = `${identity}|${timesKey}|${cut}|${themeEpoch}|${cs.colorPrevClose}|${up}|${down}`;
		if (dataKey !== renderedDataKey) {
			renderedDataKey = dataKey;
			const shift = logicalRangeShift(renderedTimes, barTimes);
			// Wrapped because a page of older history renumbers every bar:
			// without shifting the visible range by as far as they moved, the
			// chart jumps back by the size of the page the moment it lands. A
			// window sharing no bar with its predecessor reports no shift at
			// all, and is framed below instead.
			holdViewport(
				() => {
					series?.setData(
						window.map((b, i) => {
							const base = {
								time: Math.floor(b.ts_open / 1_000_000_000) as UTCTimestamp,
								open: b.open,
								high: b.high,
								low: b.low,
								close: b.close
							};
							if (!cs.colorPrevClose) return base;
							// Against the previous bar's close rather than this bar's own
							// open — the whole point of the setting.
							const previous = i > 0 ? window[i - 1].close : b.open;
							const rising = b.close >= previous;
							return {
								...base,
								color: rising ? up : down,
								borderColor: rising ? up : down,
								wickColor: rising ? up : down
							};
						})
					);
				},
				shift
			);
			// A window that shares no bar with the one it replaced is not this
			// pane scrolling — it is different data. The frame computed for the
			// old bars describes nothing that is on screen any more, so it has to
			// be computed again. Keying the guard on `instrument|spec` alone let
			// the *first* window a pane received frame it permanently: on a reload
			// the chart could be framed against one dataset, have a second replace
			// it a moment later, and keep the first one's frame for good. That is
			// the flash of unfamiliar bars a reader sees just before the real ones
			// arrive, and why the wrong frame outlived them.
			if (shift === null && renderedTimes.length > 0) framedFor = '';
			renderedTimes = barTimes;
			// The status line reads the newest bar whenever the pointer is not
			// over the plot, so a new window has to refresh it — otherwise the
			// header keeps describing bars the pane no longer shows.
			reportLatestStatusBar();
			// Replay moved to a different bar: follow the revealed edge,
			// keeping the reader's own zoom. Holding the frame completely
			// still here would be the wrong reading of the same rule that is
			// right for a page of older history — there the bars the reader is
			// watching stay put, here the whole point is that the edge moves.
			if (replayIdx != null && cut !== renderedCut) {
				chart.timeScale().scrollToPosition(Math.max(0, cs.marginRight), false);
			}
			renderedCut = replayIdx == null ? null : cut;
		}
		if (timesKey !== publishedBarTimes) {
			publishedBarTimes = timesKey;
			onBarTimes?.(barTimes);
		}
		const framePriceScale = shouldFramePriceScale({
			hasBars: bars.length > 0,
			alreadyFramed: framedFor === identity,
			storedAutoScale: cs.autoScale
		});
		if (bars.length === 0 || framedFor === identity) return;
		framedFor = identity;
		frameDefaultView();
		// The time axis is only half of framing a new series. A stored
		// `autoScale: false` was applied to this scale while it still had no
		// bars, which freezes it on an empty chart — the bars then land
		// outside that range and the pane reads as blank until it is
		// auto-fitted by hand. Whether that happened is a race: bars served
		// from cache arrive before the settings are applied and the pane
		// looks fine, bars fetched over the network arrive after and it does
		// not.
		//
		// Forcing auto on used to be rejected here because it left the chart
		// auto-fitting while the stored setting said otherwise, so the axis
		// shortcut reported a state the chart was not in. That objection is
		// answered by moving the setting with it — the same read-back the
		// axis drag already does, in the other direction.
		if (framePriceScale) {
			series.priceScale().applyOptions({ autoScale: true });
			onAutoScaleChanged?.(true);
		}
	});

	// Live price: a lease taken through the WS endpoint the
	// same way a chart pane always should — `crates/api/src/ws.rs`'s own
	// doc is explicit that the lease stays a `Drop` guard all the way up.
	// Subscribing here and unsubscribing in this effect's own cleanup is
	// this pane's whole half of that: closing the pane, or switching its
	// instrument, always runs the cleanup below, which is what actually
	// releases the server-side lease — nobody has to remember to call
	// anything else.
	$effect(() => {
		const topic = instrument;
		wsClient.subscribe(topic);
		livePrice = null;
		// A fresh subscribe on a new instrument starts from "not yet told
		// otherwise" — the previous instrument's `unsupported` answer (if
		// any) says nothing about this one.
		unsupported = false;
		onLivePrice?.(null);
		return () => wsClient.unsubscribe(topic);
	});

	// Folds each live tick into the still-forming last bar — the same bar
	// this pane is already rendering, kept current between full reloads
	// rather than waiting for the next one. Skipped during replay: a
	// scripted playback must not have a live tick jump its candles ahead of
	// the position being replayed.
	//
	// `bars`/`series`/`replayIdx` are read inside `untrack` deliberately:
	// this effect's only reactive dependency is the incoming WS event
	// (`wsEventsStore.last`) and `instrument`. Without `untrack`, reading
	// `bars` here would make this effect re-run every time `bars` changes —
	// including from the write this same effect would otherwise make to
	// it, which is exactly the update-depth-exceeded loop this was caught
	// doing live before `untrack` was added. `series.update` already
	// re-renders the chart imperatively; the reactive `bars` array itself
	// is intentionally left holding the last *fetched* close, not the live
	// one, since nothing else reads it as "the live price."
	$effect(() => {
		const event = wsEventsStore.last;
		// The authoritative half of `liveState`: a definitive per-topic
		// answer, so once set it is never cleared here — only a fresh
		// subscribe (a new instrument, above) starts asking again.
		if (isUnsupportedFor(event, instrument)) unsupported = true;
		const quote = quoteFromEvent(event, `quote:${instrument}`);
		if (quote != null) liveQuote = quote;
		const price = priceFromEvent(event, instrument);
		if (price == null) return;
		livePrice = price;
		onLivePrice?.(price);
		untrack(() => {
			if (replayIdx != null || !series || bars.length === 0) return;
			const stepNanos = (TF_DURATION_SECONDS[spec as Timeframe] ?? 3600) * 1_000_000_000;
			const last = bars[bars.length - 1];

			// Which bar this tick belongs to. Derived from the server's own
			// `next_bar_open_at` rather than by flooring the clock, so a
			// venue-anchored daily series rolls over on its own boundary and
			// not at UTC midnight. Without this the newest candle simply grew
			// forever: every tick folded into the same fetched bar and a bar
			// closing never started a new one.
			//
			// Computed, not stepped. Advancing one bar at a time held fine
			// while the pane was always showing "now" and fell apart the
			// moment it was not: a pane jumped back to a date years ago spun
			// that loop millions of times on the very first tick to arrive.
			let bucketOpen = last.ts_open;
			const nowNanos = Date.now() * 1_000_000;
			if (nextBarOpenAt != null) {
				const anchor = nextBarOpenAt - stepNanos;
				const steps = Math.max(0, Math.floor((nowNanos - anchor) / stepNanos));
				bucketOpen = anchor + steps * stepNanos;
			}

			// A live tick only belongs on this chart while the pane is
			// actually showing the live edge. After a jump to an older date it
			// is not, and folding "now" in anyway appends a candle years past
			// the loaded window — which stretches the time axis across the
			// whole gap and leaves the bars the reader asked for squeezed
			// against the left edge of what then reads as an empty chart.
			//
			// A pane that has merely fallen a few bars behind is a different
			// case with a different answer: ask the loader for the bars it is
			// missing, once per bar, rather than either drawing over the hole
			// or going quiet for good.
			// A boundary older than the newest bar already drawn means the
			// server's `next_bar_open_at` and the bars it served disagree.
			// `series.update` refuses a point older than the last one, so
			// there is nothing to do here but leave the candle alone.
			if (bucketOpen < last.ts_open) return;
			if (bucketOpen > last.ts_open + stepNanos) {
				const missingBars = (bucketOpen - last.ts_open) / stepNanos;
				if (missingBars <= HISTORY_PAGE_BARS && catchUpRequestedFor !== bucketOpen) {
					catchUpRequestedFor = bucketOpen;
					barEpoch += 1;
				}
				return;
			}

			if (nextBarOpenAt != null && bucketOpen + stepNanos !== nextBarOpenAt) {
				nextBarOpenAt = bucketOpen + stepNanos;
				// A bar closed. Re-read it from the loader so the candle
				// left behind is the venue's own, not one assembled from
				// whatever ticks this client happened to see.
				barEpoch += 1;
			}

			if (liveCandle == null || liveCandle.ts_open !== bucketOpen) {
				liveCandle =
					bucketOpen === last.ts_open
						? {
								...last,
								close: price,
								high: Math.max(last.high, price),
								low: Math.min(last.low, price)
							}
						: {
								ts_open: bucketOpen,
								open: price,
								high: price,
								low: price,
								close: price
							};
			} else {
				liveCandle.high = Math.max(liveCandle.high, price);
				liveCandle.low = Math.min(liveCandle.low, price);
				liveCandle.close = price;
			}

			series.update({
				time: Math.floor(liveCandle.ts_open / 1_000_000_000) as UTCTimestamp,
				open: liveCandle.open,
				high: liveCandle.high,
				low: liveCandle.low,
				close: liveCandle.close
			});
			onLastClose?.(price);
			// The header reads the newest bar while the pointer is away, so a
			// tick has to reach it too — otherwise the status line sits on the
			// last *fetched* close while the badge beside it moves.
			reportLatestStatusBar();
		});
	});

	// Best bid and offer are visible only when explicitly enabled and the
	// current source reports the quote capability. Removing either guard must
	// make the settings-capability test fail: a quote-shaped decoration on a
	// venue that cannot report quotes would be false market data.
	$effect(() => {
		if (!series) return;
		for (const line of quoteLines) {
			try {
				series.removePriceLine(line);
			} catch {
				// The series can be torn down while an effect is queued.
			}
		}
		quoteLines = [];
		if (!paneSettings.bidAsk || hasQuoteFeed(instrument) !== true || liveQuote == null) return;
		const T = themeColors(isDark());
		quoteLines = [
			series.createPriceLine({
				price: liveQuote.bid,
				color: T.gain,
				lineWidth: 1,
				lineStyle: 2,
				axisLabelVisible: true,
				title: 'BID'
			}),
			series.createPriceLine({
				price: liveQuote.ask,
				color: T.loss,
				lineWidth: 1,
				lineStyle: 2,
				axisLabelVisible: true,
				title: 'ASK'
			})
		];
	});

	// Overlay indicators: SMA/EMA/WMA/BollingerBands/Vwap/Atr
	// draw as extra line series on this same price scale; Volume draws as a
	// histogram pinned to the chart's own bottom margin via a dedicated
	// (unlabelled) price scale — otherwise its raw values would dwarf the
	// candlesticks' y-range.
	//
	// Reconciled from scratch on every run, but — unlike the bars effect —
	// this used to have no generation guard at all, on the theory that the
	// overlay set only changes on an edit that already re-renders this whole
	// component. A visibility toggle mutates `overlayLayers` in place
	// without doing that, so two rapid toggles produced two overlapping
	// async runs racing over the shared `overlaySeries` map — `indicatorGeneration`
	// closes that the same way `loadGeneration` already does for bars.
	//
	// A hidden layer is no longer filtered out of `layers` and dropped from
	// `overlaySeries` (which meant every hide/show destroyed and rebuilt its
	// series — the exact window the race needed): it stays reconciled with
	// `visible: false` instead, so hiding and re-showing a layer costs an
	// option flip, not a teardown.
	$effect(() => {
		if (!chart) return;
		// An indicator is derived from the bars, so it may not be drawn
		// before them. Loading the two independently is right and stays —
		// that is what keeps a slow indicator from holding up the candles —
		// but the price series resolving second used to leave an EMA hanging
		// over an empty chart, a reading with its own source not yet on
		// screen. Tracked by length so this re-runs the moment bars arrive.
		if (bars.length === 0) return;
		const currentInstrument = instrument;
		const currentSpec = spec;
		// Read synchronously (not inside the `.then()` below) so this effect
		// is a *reactive dependent* of `priceScale` and re-runs once the bars
		// effect above sets the real value — `priceScale` starts at its `0`
		// default, and on a fresh mount this effect's `loadIndicatorSeries`
		// call can resolve before that happens (it resolves from whatever is
		// already cached, with no polling to wait out), which used to plot
		// every price-scaled field undivided (a Sma/Ema/.../Atr value in the
		// millions, dragging the whole price axis and squashing the
		// candlesticks flat) and then never correct itself, since a value
		// read inside an async callback is invisible to Svelte's dependency
		// tracker.
		const currentPriceScale = priceScale;
		const currentQtyScale = qtyScale;
		// Tracked by value; the array is read untracked below.
		const signature = overlaySignature;
		// Tracked so this effect re-runs the moment a dynamic indicator's
		// plugin is enabled or disabled elsewhere — see `indicatorCatalog`'s
		// own doc comment. A layer outside this set is filtered out below,
		// which lets the existing "not seen -> removed" cleanup already in
		// this effect turn it into a placeholder with no code of its own:
		// its series (if any) is torn down the same way a deleted layer's
		// already is, and its own `indicatorName`/`params` are untouched
		// (they live on the layer itself, never on this component's state).
		const catalog = indicatorCatalog;
		const token = indicatorGeneration.begin();
		const layers = untrack(() =>
			overlayLayers.filter((l) => l.kind === 'indicator_overlay' && l.indicatorName)
		).filter((l) => catalog.has(l.indicatorName as string));
		const range = loadedRange;
		if (!range) return;
		const { from, to } = range;
		const cut = replayIdx == null ? Infinity : replayIdx;

		// Loading is about the *range*, not about every re-run. The still-
		// forming bar's own newest point no longer comes from this batch load
		// at all — the live indicator subscription below streams it directly
		// — so this effect only re-runs on a real change: a new indicator,
		// changed inputs, a different timeframe, or older bars coming into
		// view.
		const loadKey = `${currentInstrument}|${currentSpec}|${from}|${to}|${signature}`;
		if (loadKey !== lastLoadedOverlays) reportLoading(layers.map((l) => l.id));

		Promise.all(
			layers.map(async (layer) => {
				const { byField, display } = await loadIndicatorSeries(
					currentInstrument,
					currentSpec,
					from,
					to,
					layer.indicatorName as string,
					layer.params
				);
				return { layer, byField, display };
			})
		)
			.then((results) => {
				if (destroyed || !chart || !indicatorGeneration.isCurrent(token)) return;
				const seen = new Set<string>();
				let hasVolumeOverlay = false;
				// Every overlay layer's non-series display objects (`Segment`,
				// `Level`, `Box`, `Label`), converted once here and handed to the
				// pane's one shared renderer — `drawing-primitive.ts`'s own doc on
				// why a persisted drawing and an indicator's object never need a
				// second render path. Replaced wholesale on every resolve, so a
				// removed or edited layer's stale objects never linger.
				drawingsPrimitive?.setIndicatorObjects(
					results.flatMap(({ layer, display }) => objectDrawablesFromIndicatorDisplay(display, layer.id))
				);
				for (const { layer, byField } of results) {
					const plots = plotsForLayer(layer.id, layer.indicatorName as string);
					for (const plot of plots) {
						const points = byField.get(plot.field) ?? [];
						if (points.length === 0) continue;
						const key = `${layer.id}:${plot.field}`;
						seen.add(key);
						const scaleKind = indicatorFieldScale(layer.indicatorName as string);
						const divisor = scaleKind === 'price' ? 10 ** currentPriceScale : scaleKind === 'qty' ? 10 ** currentQtyScale : 1;
						let handle = overlaySeries.get(key);
						if (!handle) {
							handle =
								plot.type === 'histogram'
									? chart!.addSeries(HistogramSeries, {
											priceScaleId: 'senken-volume-overlay',
											priceFormat: { type: 'volume' },
											// The volume overlay shares the price axis but is not a
											// price: its own last-value label would sit among the
											// price ticks reading something like `3.02K`.
											lastValueVisible: false,
											priceLineVisible: false
										})
									: chart!.addSeries(LineSeries, { priceLineVisible: false, lastValueVisible: false });
							overlaySeries.set(key, handle);
						}
						if (plot.type === 'histogram') hasVolumeOverlay = true;
						const visible = layer.visible && plot.visible;
						handle.applyOptions(
							plot.type === 'line'
								? { color: plot.color, lineWidth: plot.width as 1 | 2 | 3 | 4, lineStyle: LINE_STYLE_MAP[plot.style], visible }
								: { color: plot.color, visible }
						);
						const windowed = Number.isFinite(cut) ? points.slice(0, cut) : points;
						// Held, like every other `setData` in this file. Replacing a
						// series' data is a structural update to the chart, and the
						// reader did not ask for one — showing or hiding a layer must
						// change what is drawn and nothing else. This path is the one an
						// overlay indicator takes, so it is the one a hidden EMA moved
						// the viewport through.
						holdViewport(() => {
							handle!.setData(
								windowed.map((p) => ({ time: p.time as UTCTimestamp, value: p.value / divisor }))
							);
						});
					}
				}
				for (const [key, handle] of overlaySeries) {
					if (!seen.has(key)) {
						try {
							chart!.removeSeries(handle);
						} catch {
							// chart already torn down
						}
						overlaySeries.delete(key);
					}
				}
				if (hasVolumeOverlay) {
					chart!.priceScale('senken-volume-overlay').applyOptions({ scaleMargins: { top: 0.82, bottom: 0 }, visible: false });
				}
			})
			.finally(() => {
				if (indicatorGeneration.isCurrent(token)) {
					lastLoadedOverlays = loadKey;
					reportLoading([]);
				}
			})
			.catch(() => {
				// A failed indicator fetch should not take the candlesticks down
				// with it; the layer simply shows nothing until it succeeds.
			});
	});

	// A second instrument overlaid on this pane (`addInstrumentOverlay` ->
	// an `overlay_instrument` layer): one line series per such layer, drawn
	// from that instrument's own bars over the pane's currently loaded range
	// — never the mock candle series, and never the main instrument's price
	// scale (an overlay instrument's prices are unrelated to the main one's;
	// see `overlayInstrumentCloseSeries`'s own doc). Reconciled the same way
	// as the indicator-overlay effect just above: a `GenerationGuard` so two
	// rapid adds/removes cannot race over the shared series map, a
	// `destroyed` check before touching `chart`/`series` in the `.then()`,
	// and a hidden layer keeping its series (just flipped `visible: false`)
	// rather than being torn down and rebuilt.
	$effect(() => {
		if (!chart || bars.length === 0) return;
		const currentSpec = spec;
		// Tracked by value; `overlayLayers` itself is read untracked below —
		// same reasoning as `overlaySignature`/`signature` just above.
		const signature = instrumentOverlaySignature;
		const token = overlayInstrumentGeneration.begin();
		const plan = untrack(() => planOverlayInstrumentSeries(overlayLayers));
		const range = loadedRange;
		if (!range) return;
		const { from, to } = range;
		void signature;

		Promise.all(
			plan.map(async (item) => {
				const resolved = await loadBars(item.instrument, currentSpec, from, to, undefined, () => !destroyed);
				return { item, resolved };
			})
		)
			.then((results) => {
				if (destroyed || !chart || !overlayInstrumentGeneration.isCurrent(token)) return;
				for (const { item, resolved } of results) {
					// The overlay instrument's *own* price scale, from its *own*
					// `loadBars` response — never `priceScale` (the main pane's
					// state variable), which is what `overlayInstrumentCloseSeries`
					// exists to make impossible to mix up by accident.
					const points = overlayInstrumentCloseSeries(resolved.bars, resolved.priceScale);
					let handle = overlayInstrumentSeries.get(item.layerId);
					if (!handle) {
						handle = chart!.addSeries(LineSeries, {
							priceScaleId: item.priceScaleId,
							priceLineVisible: false,
							lastValueVisible: false
						});
						overlayInstrumentSeries.set(item.layerId, handle);
						// A dedicated, unlabelled scale per overlay instrument: it
						// exists only so this series autoscales independently of the
						// candles (see `overlayInstrumentPriceScaleId`'s doc) and must
						// never surface as a second visible axis.
						chart!.priceScale(item.priceScaleId).applyOptions({ visible: false });
					}
					const [plot] = plotsForLayer(item.layerId, OVERLAY_INSTRUMENT_PLOT);
					if (plot) {
						handle.applyOptions({
							color: plot.color,
							lineWidth: plot.width as 1 | 2 | 3 | 4,
							lineStyle: LINE_STYLE_MAP[plot.style],
							visible: item.visible && plot.visible
						});
					}
					// Held, for exactly the reason the comment here used to give for
					// *not* holding it: an added overlay series must not move the
					// reader's viewport. Leaving it unwrapped is what allows the
					// move — wrapping is what prevents it. An overlaid instrument's
					// times need not match the candles', so replacing its data can
					// change the chart's extent and renumber every logical index.
					holdViewport(() => {
						handle!.setData(points.map((p) => ({ time: p.time as UTCTimestamp, value: p.value })));
					});
				}
				for (const key of overlaySeriesKeysToRemove(plan, overlayInstrumentSeries.keys())) {
					const handle = overlayInstrumentSeries.get(key);
					if (handle) {
						try {
							chart!.removeSeries(handle);
						} catch {
							// chart already torn down
						}
					}
					overlayInstrumentSeries.delete(key);
				}
			})
			.catch(() => {
				// A failed overlay-instrument fetch should not take the
				// candlesticks down with it; the layer simply shows nothing
				// until it succeeds.
			});
	});

	/** Every overlay/sub-pane layer currently plotting an indicator,
	 * combined — one live indicator session per layer. As a joined string
	 * rather than the raw arrays, for the same reason `overlaySignature`
	 * above is: the parent re-renders (handing this component fresh array
	 * identities) on every live tick, and the subscribe effect below must
	 * only actually resubscribe on a real change of layer/indicator/params,
	 * not on every render. Visibility is deliberately excluded — a hidden
	 * layer's session stays open, the same "an option flip, not a teardown"
	 * choice the batch reconciliation above already makes for its series. */
	const liveIndicatorSignature = $derived(
		[
			...overlayLayers.filter((l) => l.kind === 'indicator_overlay' && l.indicatorName),
			...subPaneLayers.filter((l) => l.indicatorName)
		]
			.map((l) => `${l.id}:${l.indicatorName}:${JSON.stringify(l.params)}`)
			.join('|')
	);

	/** This pane's currently-open live indicator sessions, keyed by layer id
	 * — so the frame-routing effect below knows which series handle(s) an
	 * incoming `indicator` frame belongs to. Reassigned wholesale by the
	 * subscribe effect, never mutated in place. */
	let liveIndicatorTopics = new Map<string, string>();

	// Opens (or, ref-counted by `wsClient` the same way a price/quote lease
	// already is, joins) exactly one live indicator session per layer
	// currently plotting an indicator — the WS counterpart to the two batch
	// `loadIndicatorSeries` effects above, replacing the once-a-second
	// recompute those used to need to stay current. Every topic this run
	// opens is closed in this same effect's own cleanup, so a pane that
	// closes, an indicator that is removed or edited, or this pane's
	// instrument/timeframe changing never leaks a session — the same
	// subscribe-on-entry/unsubscribe-on-cleanup shape the live-price and
	// quote effects below already use, just for a variable-length set of
	// topics instead of one.
	$effect(() => {
		const currentInstrument = instrument;
		const currentSpec = spec;
		// Tracked by value; the arrays themselves are read untracked below.
		void liveIndicatorSignature;
		const layers = untrack(() => [
			...overlayLayers.filter((l) => l.kind === 'indicator_overlay' && l.indicatorName),
			...subPaneLayers.filter((l) => l.indicatorName)
		]);
		const range = loadedRange;
		if (!range) return;
		const { from, to } = range;
		const opened = new Map<string, string>();
		for (const layer of layers) {
			const request: IndicatorSubscribeRequest = {
				instrument: currentInstrument,
				spec: currentSpec,
				indicator: layer.indicatorName as string,
				params: JSON.stringify(layer.params),
				from,
				to
			};
			opened.set(layer.id, wsClient.subscribeIndicator(request));
		}
		liveIndicatorTopics = opened;
		return () => {
			for (const topic of opened.values()) wsClient.unsubscribe(topic);
		};
	});

	// Applies each live indicator reading to its series' newest point the
	// moment it arrives, the same imperative `series.update` the candle's
	// own tick-fold effect above uses — a value that is slow to come back
	// only ever makes itself late, never holds up anything else. A
	// `provisional: true` reading is drawn exactly like a settled one (the
	// visual treatment `POST /api/indicators/compute`'s own provisional bar
	// already had): nothing here — or downstream of it — ever treats it as
	// final, since the next reading for the same still-forming bar simply
	// overwrites this same point again, and the bar's actual close arrives
	// as its own `provisional: false` reading.
	$effect(() => {
		const event = wsEventsStore.last;
		if (!event || event.type !== 'indicator') return;
		untrack(() => {
			const currentPriceScale = priceScale;
			const currentQtyScale = qtyScale;
			// Held, and the asymmetry with the candle update above is the point.
			//
			// `shiftVisibleRangeOnNewBar` is on by default, so giving a series a
			// time point it does not yet have scrolls the chart right by one bar.
			// The candles want exactly that — a new bar should carry the reader
			// forward. A derived line must not: an indicator batch ends at the
			// last *closed* bar, so the first live reading for the forming bar is
			// a new point on that series and shifts the view — again on every
			// tick. It is why the viewport crept away from the newest bar only on
			// panes carrying an indicator, and why a bare chart was fine.
			holdViewport(() => {
				for (const [layerId, topic] of liveIndicatorTopics) {
					const reading = indicatorFrameFor(event, topic);
					if (!reading) continue;
					const handle = overlaySeries.get(`${layerId}:${reading.field}`) ?? subPaneSeries.get(`${layerId}:${reading.field}`);
					if (!handle) continue;
					const layer = overlayLayers.find((l) => l.id === layerId) ?? subPaneLayers.find((l) => l.id === layerId);
					const scaleKind = indicatorFieldScale(layer?.indicatorName ?? '');
					const divisor = scaleKind === 'price' ? 10 ** currentPriceScale : scaleKind === 'qty' ? 10 ** currentQtyScale : 1;
					handle.update({
						time: Math.floor(reading.ts_open / 1_000_000_000) as UTCTimestamp,
						value: reading.value / divisor
					});
				}
			});
		});
	});

	// This pane's live-price state — combines the `GET /api/sources`
	// cache (checked before a subscribe even goes out) with this pane's own
	// `unsupported` flag (the WS layer's authoritative per-topic answer,
	// set above). `hasLiveFeed` returns `undefined` while the sources cache
	// has not resolved yet, which `deriveLiveState` treats the same as
	// "live" — a pane must never show a "no feed" banner on the strength of
	// an answer it has not actually received.
	const liveState = $derived(deriveLiveState(hasLiveFeed(instrument), unsupported, marketStatus));

	/** Which way the price is going, for the badge and the price line.
	 *
	 * Measured against the last closed bar, not that bar's own open/close:
	 * this pane plots closed bars only, so the newest candle is already
	 * finished while the live price has moved on past it. Colouring by the
	 * finished candle makes the badge disagree with the number printed
	 * inside it — a price above the last close shown in red.
	 *
	 * `flat` whenever there is no live feed: with nothing moving, a
	 * direction would be a claim about a market this pane is not watching. */
	const barDirection = $derived.by((): PriceDirection => {
		if (!showCountdown(liveState) || bars.length === 0) return 'flat';
		const last = bars[bars.length - 1];
		const reference = livePrice != null ? last.close : last.open;
		const current = livePrice ?? last.close;
		if (current > reference) return 'up';
		if (current < reference) return 'down';
		return 'flat';
	});

	// The candles here are deliberately monochrome, so the library's own
	// "price line takes the last bar's colour" default cannot express
	// direction — it would paint the line white. The direction colour is
	// therefore set explicitly, and dropped to the neutral token the moment
	// the pane stops streaming, so a still chart never shows a live-looking
	// green or red line.
	$effect(() => {
		if (!series) return;
		const T = themeColors(isDark());
		const color =
			barDirection === 'up' ? T.gain : barDirection === 'down' ? T.loss : priceLineColorFor(liveState);
		series.applyOptions({ priceLineColor: color });
	});

	// The countdown belongs on screen only in `live` state — a countdown
	// against a feed that is not running would be a lie. Gated here, not by
	// attaching/detaching the primitive itself, so the primitive's own
	// one-second timer has one clear owner.
	$effect(() => {
		const deadline = nextBarOpenAt;
		if (deadline == null || !showCountdown(liveState) || !paneSettings.countdown) {
			priceBadge?.clearDeadline();
			return;
		}
		const stepSeconds = TF_DURATION_SECONDS[spec as Timeframe] ?? 3600;
		priceBadge?.setDeadline(deadline, stepSeconds);
	});

	// Keeps the countdown label's price-axis coordinate anchored to this
	// pane's current price — the live tick once one has arrived for this
	// instrument, otherwise the last fetched close (the same fallback
	// `pane-header.svelte`'s own live-price display uses).
	$effect(() => {
		const price = livePrice ?? (bars.length > 0 ? bars[bars.length - 1].close : null);
		priceBadge?.setPrice(paneSettings.symbolLabel === 'HIDDEN' ? null : price, priceScale, barDirection);
	});

	// The floating overlay naming the reason there is no countdown/coloured
	// line — `null` in `live` state, so this and the countdown are
	// never both on screen (the whole reason `liveState` is one union
	// rather than independent flags).
	// Reported to the pane header rather than drawn over the chart: this is
	// a standing fact about the pane, not a passing event, and the chart's
	// top-left corner already belongs to the layer chips.
	/** The header's live-state chip: its short label and the sentence behind
	 * it. `null` while the venue streams — the common case must not put a
	 * permanent badge in the header. */
	const liveChip = $derived.by(() => {
		const label = liveChipLabel(liveState);
		const message = overlayMessage(liveState, userZoneStore.zone);
		return label && message ? { label, message } : null;
	});
	$effect(() => {
		onLiveNotice?.(liveChip);
	});

	// One source for "this pane is fetching bars": the load progress the pane
	// already tracks, plus the separate older-history path. Reported rather
	// than duplicated as a second flag, so the two can never disagree.
	$effect(() => {
		const phase = progress.phase;
		onBarsLoading?.(loadingOlder || phase === 'checking' || phase === 'fetching');
	});

	const progressLabel = $derived.by(() => {
		if (progress.phase === 'fetching') {
			const secs = progress.job?.estimate_secs ?? progress.requirement.estimate_secs;
			return secs != null ? `FETCHING · ~${Math.ceil(secs)}S` : 'FETCHING…';
		}
		// The raw message is a transport string — a full request URL with its
		// query string and an HTTP status code. It goes to the console
		// (see the bars-loading effect above), never the screen.
		if (progress.phase === 'error') return 'ERROR · COULD NOT LOAD THIS CHART';
		return null;
	});

	/** The pane's one transient status line, or `null` when there is nothing
	 * to say.
	 *
	 * Deliberately a single value rather than two independent badges: a
	 * fetch in flight used to render `LOADING…` and `FETCHING · ~18S`
	 * stacked on top of each other, which is the same statement twice, in
	 * two boxes. Where both apply the more specific one wins — a running
	 * estimate tells the reader everything "loading" would, and more.
	 *
	 * While `bars` is empty, "still working" and "finished, and there is
	 * genuinely nothing here" must still read differently: both used to be
	 * the same blank canvas. */
	const paneStatus = $derived.by((): string | null => {
		if (progressLabel) return progressLabel;
		if (bars.length > 0) return null;
		return progress.phase === 'ready' ? 'NO DATA FOR THIS RANGE' : 'LOADING…';
	});

</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	bind:this={container}
	data-chart-pane
	data-native-pane-count={1 + subPaneLayers.length}
	data-shared-crosshair="true"
	class="absolute inset-0"
	oncontextmenu={(e) => {
		if (!onContextMenu || !container) return;
		e.preventDefault();
		const box = container.getBoundingClientRect();
		const fromRight = box.right - e.clientX;
		const fromBottom = box.bottom - e.clientY;
		// The price and time scales are the chart library's own gutters, not
		// separate elements, so which one was clicked has to come from the
		// pointer's distance to the edge.
		const region =
			fromRight <= PRICE_SCALE_WIDTH_PX
				? 'price-scale'
				: fromBottom <= TIME_SCALE_HEIGHT_PX
					? 'time-scale'
					: 'chart';
		onContextMenu({ x: e.clientX, y: e.clientY, region });
	}}
></div>
<!-- Transient pane progress (loading, fetching/backfill, no data for this
     range) — anchored bottom-centre, above the time axis, rather than
     scattered between the chart's centre and its top-right corner: none of
     it is a standing fact about the pane (that is the header's live-state
     chip's job), it is always temporary, and this is the one spot nothing
     else on the chart ever draws into. -->
<div class="pointer-events-none absolute inset-x-0 bottom-[3.25rem] z-[9] flex flex-col items-center">
	{#if paneStatus}
		<span class="border border-ink/14 bg-bg2 px-2.5 py-1 font-mono text-[9px] tracking-[0.18em] text-secondary-foreground">{paneStatus}</span>
	{/if}
</div>
<HistoryEdgeStatus edgeState={historyEdgeState} />
