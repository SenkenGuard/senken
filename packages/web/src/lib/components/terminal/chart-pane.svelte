<script lang="ts">
	// One pane's lightweight-charts candlestick canvas (`data-chart-pane` mount points at line 450, chart construction at
	// lines 1736-1801, theme palette at chartTheme() line 1917, replay
	// slicing at applyReplay() line 2088).
	//
	// Drawing objects (horizontal line, trend line, rectangle) are rendered
	// and hit-tested by `$lib/charts/drawing-primitive.ts`'s
	// `DrawingsPrimitive`, attached to this pane's candlestick series — not
	// the reference's own click-to-paint `IPriceLine`s (line 1780), which
	// have no identity a later click could select or edit.
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
	import { CandlestickSeries, HistogramSeries, LineSeries, createChart, createTextWatermark, type IChartApi, type ISeriesApi, type ITextWatermarkPluginApi, type Time, type UTCTimestamp } from 'lightweight-charts';
	import {
		OBJECT_DRAW_TOOLS,
		TF_DURATION_SECONDS,
		type Timeframe,
		type ToolKey,
		isIntradaySpec,
		timeAxisFormatter
	} from './chart-config';
	import { loadBars, loadIndicatorSeries, type BarLoadProgress, type ProvisionalBar } from '$lib/charts/bars';
	import { GenerationGuard } from '$lib/charts/generation-guard';
	import { indicatorFieldScale, plotsForLayer, LINE_STYLE_MAP } from '$lib/charts/layer-style';
	import type { DrawingRuntime, LayerRuntime } from '$lib/charts/pane-runtime';
	import { DrawingsPrimitive } from '$lib/charts/drawing-primitive';
	import { drawingAt } from '$lib/charts/drawing-layer';
	import {
		initialBarWindow,
		historyLoadPriority,
		HISTORY_PAGE_BARS,
		previousHistoryPage,
		preserveVisibleRange,
		shouldPrefetchHistory
	} from '$lib/charts/chart-window';
	import { nativePaneData } from '$lib/charts/native-panes';
	import { shouldFramePriceScale } from '$lib/charts/pane-settings';
	import { LastPriceBadgePrimitive, type PriceDirection } from '$lib/charts/price-badge-primitive';
	import { deriveLiveState, overlayMessage, priceLineColorFor, showCountdown } from '$lib/charts/live-state';
	import type { MarketStatus } from '$lib/charts/live-state';
	import { chartPaneThemeColors, isDarkTheme } from './chart-theme';
	import { defaultChartSettings, type ChartSettings } from '$lib/mock/chart-settings';
	import { wsClient } from '$lib/api/websocket';
	import { wsEventsStore, priceFromEvent, qtyFromEvent } from '$lib/api/ws-events.svelte';
	import { isUnsupportedFor, quoteFromEvent } from '$lib/api/ws-frames';
	import { ensureSourcesLoaded, hasLiveFeed, hasQuoteFeed } from '$lib/api/sources.svelte';

	/** A freshly drawn object's starting style — the same default plot colour
	 * `$lib/charts/layer-style.ts` already uses, so a new drawing and a new
	 * indicator plot look consistent on a fresh chart. */
	const DEFAULT_DRAWING_STYLE = { color: '#f2f2ef', width: 2, lineStyle: 'SOLID' as const };

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
		onCrosshair,
		onNarrow,
		onLastClose,
		onLiveNotice,
		onPriceScale,
		onLivePrice,
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
		 * layout (`clearAllDrawings`, mirroring the reference's own
		 * `clearDrawings`, line 2126). */
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
		/** Fires the crosshair's formatted OHLC/close text for the pane header
		 * status line (subscribeCrosshairMove, line 1757). */
		onCrosshair?: (text: string) => void;
		/** Fires whenever the container crosses the 330px width the reference
		 * uses to hide the venue prefix (paneNarrow, line 1794). */
		onNarrow?: (narrow: boolean) => void;
		/** Fires with the most recent bar's real decimal close, whenever bars
		 * (re)load — the order ticket's `mid` price comes from here now,
		 * instead of a mock candle series. */
		onLastClose?: (price: number) => void;
		onLiveNotice?: (notice: string | null) => void;
		/** Fires with `price_scale` whenever bars (re)load. */
		onPriceScale?: (scale: number) => void;
		/** Fires with the live last-traded price, or `null`
		 * once this pane's instrument changes and no tick has arrived yet
		 * for the new one. `pane-header.svelte` shows this next to the
		 * instrument chip whenever the crosshair isn't overriding it. */
		onLivePrice?: (price: number | null) => void;
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
		 * tool (the owner's own acceptance point: "after drawing a line the
		 * tool returns to the cursor"). */
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
	let watermark: ITextWatermarkPluginApi<Time> | undefined;
	let ro: ResizeObserver | undefined;
	let mo: MutationObserver | undefined;
	let lastNarrow: boolean | undefined;
	// The trend-line/rectangle tools need two clicks — this holds the first
	// point between them. `null` whenever no such drawing is in progress.
	let pendingStart: { time: number; price: number } | null = null;
	// One overlay series per (layer, field) pair — reconciled on every data
	// effect run rather than diffed, since the whole overlay set only ever
	// changes on an add/remove/edit that already re-renders this component.
	const overlaySeries = new Map<string, ISeriesApi<'Line'> | ISeriesApi<'Histogram'>>();
	const subPaneSeries = new Map<string, ISeriesApi<'Line'> | ISeriesApi<'Histogram'>>();

	let bars = $state<{ ts_open: number; open: number; high: number; low: number; close: number }[]>([]);
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
	/** The bar currently being built from live ticks — not `$state`, since it
	 * is written inside `untrack` and drawn imperatively via `series.update`. */
	let forming: {
		ts_open: number;
		open: number;
		high: number;
		low: number;
		close: number;
		volume: number;
	} | null = null;
	/** Bumped when a bar closes, so the loader is asked for the finished bar
	 * the venue actually recorded. */
	let barEpoch = $state(0);
	/** The `instrument|spec` the loaded bars belong to. */
	let loadedIdentity = '';
	/** Bumped at most once a second while the forming bar is moving, so the
	 * indicator effect re-runs on a clock of its own. The tick path never
	 * waits on a recompute: candles are drawn imperatively the instant a
	 * price arrives, and an indicator that is slow to come back only ever
	 * makes itself late. */
	let formingVersion = $state(0);
	let formingPublisher: ReturnType<typeof setInterval> | undefined;
	/** The forming bar in the wire's scaled-integer form, or `undefined` when
	 * nothing is forming yet. Prices are rounded back to the instrument's own
	 * scale here — the decimals this component works in are a display
	 * convenience, and the integer is what the series is actually defined in. */
	function provisionalForIndicators(): ProvisionalBar | undefined {
		if (forming == null) return undefined;
		const priceFactor = 10 ** priceScale;
		const qtyFactor = 10 ** qtyScale;
		return {
			ts_open: forming.ts_open,
			open: Math.round(forming.open * priceFactor),
			high: Math.round(forming.high * priceFactor),
			low: Math.round(forming.low * priceFactor),
			close: Math.round(forming.close * priceFactor),
			volume: { kind: 'real', value: Math.round(forming.volume * qtyFactor) }
		};
	}
	// Set once this pane's subscribed topic gets back an explicit
	// `unsupported` WS frame rather than a `price` tick — the authoritative
	// half of `liveState` below; `hasLiveFeed` (the `GET /api/sources`
	// cache) is the other, checked before a subscribe even goes out.
	let unsupported = $state(false);
	let progress = $state<BarLoadProgress>({ phase: 'checking' });
	// A left-edge request is deliberately independent from the primary load
	// generation. Panning into history must not cancel the bars currently on
	// screen, and its `prefetch` priority leaves visible work ahead of it.
	let loadingOlder = $state(false);
	let olderHistoryError = $state<string | null>(null);
	let earliestAvailable = $state<number | null>(null);
	let historyEdgeState = $state<'idle' | 'loading' | 'end' | 'error'>('idle');
	const loadGeneration = new GenerationGuard();
	// The indicator effect's own generation guard — `loadGeneration`
	// only covers bars, and the overlay reconciliation below races
	// independently of it: a layer hidden/shown/edited begins a new
	// generation here without touching `loadGeneration` at all, so a stale,
	// still-resolving indicator fetch can be told apart from the current one.
	const indicatorGeneration = new GenerationGuard();
	const subPaneGeneration = new GenerationGuard();
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

	function requestOlderHistory(): void {
		if (loadingOlder || destroyed || bars.length === 0) return;
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
					close: bar.close / 10 ** resolved.priceScale
				}));
				const present = new Set(bars.map((bar) => bar.ts_open));
				const appended = older.filter((bar) => !present.has(bar.ts_open));
				if (appended.length > 0) bars = [...appended, ...bars];
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
				panes: { separatorColor: T.border, separatorHoverColor: T.cross, enableResize: true }
			},
			grid: { vertLines: { color: T.grid }, horzLines: { color: T.grid } },
			rightPriceScale: { borderColor: T.border },
			timeScale: { borderColor: T.border, rightOffset: 6 },
			crosshair: {
				mode: 0,
				vertLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label },
				horzLine: { color: T.cross, width: 1, style: 2, labelBackgroundColor: T.label }
			}
		});
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
		const T = themeColors(isDark());
		chart = createChart(container, {
			layout: {
				background: { color: T.bg },
				textColor: T.axis,
				fontFamily: "'IBM Plex Mono', monospace",
				fontSize: 9,
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
			if (onCrosshair && series) {
				if (!param.point || !param.seriesData) {
					onCrosshair('');
				} else {
					const bar = param.seriesData.get(series) as { open: number; high: number; low: number; close: number } | undefined;
					if (!bar) {
						onCrosshair('');
					} else {
						const wide = (container?.clientWidth ?? 0) >= 420;
						onCrosshair(
							wide
								? `O ${bar.open.toFixed(4)} H ${bar.high.toFixed(4)} L ${bar.low.toFixed(4)} C ${bar.close.toFixed(4)}`
								: bar.close.toFixed(4)
						);
					}
				}
			}
			// Rubber-band preview of a trend line/rectangle's second point
			// while the first is already placed.
			if (pendingStart && chart && series && param.point) {
				const price = series.coordinateToPrice(param.point.y);
				const time = xToTime(param.point.x);
				if (price != null && time != null) {
					drawingsPrimitive?.setPending(pendingStart, { time, price });
				}
			}
		});

		chart.timeScale().subscribeVisibleLogicalRangeChange((range) => {
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

			if (!OBJECT_DRAW_TOOLS.includes(tool)) return;
			const price = series.coordinateToPrice(param.point.y);
			const time = xToTime(param.point.x);
			if (price == null || time == null) return;
			const point = { time, price };

			if (tool === 'hline') {
				onCreateDrawing?.({ kind: 'horizontal_line', price: point.price, ...DEFAULT_DRAWING_STYLE });
				onToolConsumed?.();
				return;
			}

			// trend line / rectangle: two clicks.
			if (!pendingStart) {
				pendingStart = point;
				drawingsPrimitive?.setPending(point, point);
				return;
			}
			onCreateDrawing?.({
				kind: tool === 'trend' ? 'trend_line' : 'rectangle',
				start: pendingStart,
				end: point,
				...DEFAULT_DRAWING_STYLE
			});
			pendingStart = null;
			drawingsPrimitive?.setPending(null, null);
			onToolConsumed?.();
		});

		// Republishes the forming bar to the indicator effect on a fixed
		// cadence rather than per tick: a busy instrument can produce many
		// ticks a second, and recomputing an indicator for each of them would
		// queue requests behind each other and make the whole pane feel late.
		let publishedClose: number | null = null;
		formingPublisher = setInterval(() => {
			if (forming == null) return;
			if (publishedClose === forming.close) return;
			publishedClose = forming.close;
			formingVersion += 1;
		}, 1000);

		ro = new ResizeObserver(() => {
			if (!container || !chart) return;
			chart.applyOptions({ width: container.clientWidth, height: container.clientHeight });
			const narrow = container.clientWidth < 330;
			if (narrow !== lastNarrow) {
				lastNarrow = narrow;
				onNarrow?.(narrow);
			}
		});
		ro.observe(container);

		mo = new MutationObserver(applyTheme);
		mo.observe(document.documentElement, { attributes: true, attributeFilter: ['class'] });

		return () => {
			destroyed = true;
			if (formingPublisher !== undefined) clearInterval(formingPublisher);
			formingPublisher = undefined;
			ro?.disconnect();
			mo?.disconnect();
			overlaySeries.clear();
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
	// which persists an empty drawing list — see `clearAllDrawings`) means a
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

	// Native panes share the chart's time scale and crosshair. Indicator
	// warm-up can therefore shorten a plot without changing which candle is
	// under its last point; there is no second logical index to synchronize.
	$effect(() => {
		if (!chart || bars.length === 0) return;
		const activeChart = chart;
		const currentInstrument = instrument;
		const currentSpec = spec;
		const currentPriceScale = priceScale;
		const layers = subPaneLayers.filter((layer) => layer.indicatorName);
		const token = subPaneGeneration.begin();
		const { from, to } = initialBarWindow(currentSpec);

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
						layer.params,
						provisionalForIndicators()
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
						preserveVisibleRange(activeChart.timeScale(), () => {
							handle.setData(nativePaneData(points, divisor));
						});
					}
				}
				for (const [key, handle] of subPaneSeries) {
					if (seen.has(key)) continue;
					activeChart.removeSeries(handle);
					subPaneSeries.delete(key);
				}
				const panes = activeChart.panes();
				const configured = settings?.paneSplit;
				for (const [index, pane] of panes.entries()) {
					const fraction = configured?.[index] ?? (index === 0 ? 65 : 35 / layers.length);
					pane.setHeight(Math.max(30, Math.round((container?.clientHeight ?? 300) * fraction / 100)));
				}
			})
			.catch(() => {
				// A failed indicator fetch leaves candles and other panes usable.
			});
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
			forming = null;
		}
		const { from, to } = initialBarWindow(currentSpec);
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
				bars = resolved.bars.map((b) => ({
					ts_open: b.ts_open,
					open: b.open / 10 ** resolved.priceScale,
					high: b.high / 10 ** resolved.priceScale,
					low: b.low / 10 ** resolved.priceScale,
					close: b.close / 10 ** resolved.priceScale
				}));
				if (bars.length > 0) onLastClose?.(bars[bars.length - 1].close);
			})
			.catch(() => {
				// `progress` already carries the error phase/message; nothing
				// further to do here.
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

	// Time-axis labels: 24h or 12h, with or without the weekday. The library
	// formats ticks itself unless given a formatter, and its default is the
	// 24h one with no weekday.
	/** What the tick formatter was last built for. A function's identity
	 * always differs, so there is nothing to compare against — and writing
	 * the time scale discards where the user has scrolled to, so this effect
	 * must not fire on every unrelated settings change. */
	let tickFormatSignature = '';

	$effect(() => {
		if (!chart) return;
		const cs = paneSettings;
		const intraday = isIntradaySpec(spec);
		const signature = `${intraday}|${cs.dayOfWeek}|${cs.timeFormat}`;
		if (signature === tickFormatSignature) return;
		chart.applyOptions({
			timeScale: { tickMarkFormatter: timeAxisFormatter(spec, cs.dayOfWeek, cs.timeFormat) }
		});
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
		chart.timeScale().fitContent();
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

	/** Length plus both ends identifies a contiguous bar window, so the
	 * timeline is only published when it really moved — a colour setting
	 * re-running the effect below must not make every sub-pane re-draw. */
	let publishedBarTimes = '';

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
		series.setData(
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
		const barTimes = window.map((b) => Math.floor(b.ts_open / 1_000_000_000) as UTCTimestamp);
		const timesKey = `${barTimes.length}|${barTimes[0] ?? ''}|${barTimes[barTimes.length - 1] ?? ''}`;
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
		chart.timeScale().fitContent();
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
			let bucketOpen = last.ts_open;
			if (nextBarOpenAt != null) {
				bucketOpen = nextBarOpenAt - stepNanos;
				const nowNanos = Date.now() * 1_000_000;
				while (nowNanos >= bucketOpen + stepNanos) bucketOpen += stepNanos;
				if (bucketOpen + stepNanos !== nextBarOpenAt) {
					nextBarOpenAt = bucketOpen + stepNanos;
					// A bar closed. Re-read it from the loader so the candle
					// left behind is the venue's own, not one assembled from
					// whatever ticks this client happened to see.
					barEpoch += 1;
				}
			}

			// Volume accumulates across the interval rather than being carried
			// over: a bar's volume is what traded *inside* it, so a new bucket
			// starts from this tick's own size and never inherits the last
			// one's total.
			const qty = qtyFromEvent(event, instrument) ?? 0;
			if (forming == null || forming.ts_open !== bucketOpen) {
				forming =
					bucketOpen === last.ts_open
						? {
								...last,
								close: price,
								high: Math.max(last.high, price),
								low: Math.min(last.low, price),
								volume: qty
							}
						: {
								ts_open: bucketOpen,
								open: price,
								high: price,
								low: price,
								close: price,
								volume: qty
							};
			} else {
				forming.high = Math.max(forming.high, price);
				forming.low = Math.min(forming.low, price);
				forming.close = price;
				forming.volume += qty;
			}

			series.update({
				time: Math.floor(forming.ts_open / 1_000_000_000) as UTCTimestamp,
				open: forming.open,
				high: forming.high,
				low: forming.low,
				close: forming.close
			});
			onLastClose?.(price);
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
		const token = indicatorGeneration.begin();
		const layers = untrack(() =>
			overlayLayers.filter((l) => l.kind === 'indicator_overlay' && l.indicatorName)
		);
		const { from, to } = initialBarWindow(currentSpec);
		const cut = replayIdx == null ? Infinity : replayIdx;

		// The forming bar, so an indicator's newest point lands on the same
		// candle the chart is drawing instead of stopping one bar behind it.
		// It carries volume as well as prices, so a VWAP or a volume
		// histogram follows the live bar like everything else.
		void formingVersion;
		const provisional = provisionalForIndicators();
		// Loading is about the *range*, not about every re-run.
		//
		// Recomputing an indicator across a bar range it has not covered
		// before is a load, and the reader should see that it is happening:
		// a new indicator, changed inputs, a different timeframe, or older
		// bars coming into view. Folding the forming bar in on each tick is
		// not — the series is already drawn and only its last point moves, so
		// a spinner there would blink once a second and mean nothing.
		//
		// The signature below deliberately excludes `formingVersion` and
		// includes the requested window, so it changes exactly when the work
		// is real.
		const loadKey = `${currentInstrument}|${currentSpec}|${from}|${to}|${signature}`;
		if (loadKey !== lastLoadedOverlays) reportLoading(layers.map((l) => l.id));

		Promise.all(
			layers.map(async (layer) => {
				const { byField } = await loadIndicatorSeries(
					currentInstrument,
					currentSpec,
					from,
					to,
					layer.indicatorName as string,
					layer.params,
					provisional
				);
				return { layer, byField };
			})
		)
			.then((results) => {
				if (destroyed || !chart || !indicatorGeneration.isCurrent(token)) return;
				const seen = new Set<string>();
				let hasVolumeOverlay = false;
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
						handle.setData(windowed.map((p) => ({ time: p.time as UTCTimestamp, value: p.value / divisor })));
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
	const liveOverlayMessage = $derived(overlayMessage(liveState));
	$effect(() => {
		onLiveNotice?.(liveOverlayMessage);
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

	const leftEdgeStatus = $derived.by((): string | null => {
		if (historyEdgeState === 'loading') return 'LOADING EARLIER HISTORY…';
		if (historyEdgeState === 'end') return 'START OF AVAILABLE HISTORY';
		if (historyEdgeState === 'error') return 'EARLIER HISTORY COULD NOT LOAD · PAN LEFT TO RETRY';
		return null;
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
<div class="pointer-events-none absolute inset-x-0 bottom-7 z-[9] flex flex-col items-center">
	{#if paneStatus}
		<span class="border border-ink/14 bg-bg2 px-2.5 py-1 font-mono text-[9px] tracking-[0.18em] text-secondary-foreground">{paneStatus}</span>
	{/if}
</div>
{#if leftEdgeStatus}
	<div class="pointer-events-none absolute bottom-7 left-2 z-[9]" data-history-edge-state={historyEdgeState}>
		<span class="border border-ink/14 bg-bg2 px-2 py-1 font-mono text-[8px] tracking-[0.14em] text-secondary-foreground">{leftEdgeStatus}</span>
	</div>
{/if}
