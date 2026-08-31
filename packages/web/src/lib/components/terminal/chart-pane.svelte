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
	import { createChart, type IChartApi, type ISeriesApi, type UTCTimestamp } from 'lightweight-charts';
	import {
		OBJECT_DRAW_TOOLS,
		TF_DURATION_SECONDS,
		type Timeframe,
		type ToolKey,
		defaultBarWindow,
		indicatorFieldScale,
		isIntradaySpec
	} from './chart-config';
	import { loadBars, loadIndicatorSeries, type BarLoadProgress } from '$lib/charts/bars';
	import { GenerationGuard } from '$lib/charts/generation-guard';
	import { plotsForLayer, LINE_STYLE_MAP } from '$lib/charts/layer-style';
	import type { DrawingRuntime, LayerRuntime } from '$lib/charts/pane-runtime';
	import { DrawingsPrimitive } from '$lib/charts/drawing-primitive';
	import { LastPriceBadgePrimitive, type PriceDirection } from '$lib/charts/price-badge-primitive';
	import { deriveLiveState, overlayMessage, priceLineColorFor, showCountdown } from '$lib/charts/live-state';
	import { chartPaneThemeColors, isDarkTheme } from './chart-theme';
	import { defaultChartSettings, type ChartSettings } from '$lib/mock/chart-settings';
	import { wsClient } from '$lib/api/websocket';
	import { wsEventsStore, priceFromEvent, qtyFromEvent } from '$lib/api/ws-events.svelte';
	import { isUnsupportedFor } from '$lib/api/ws-frames';
	import { ensureSourcesLoaded, hasLiveFeed } from '$lib/api/sources.svelte';

	/** A freshly drawn object's starting style — the same default plot colour
	 * `$lib/charts/layer-style.ts` already uses, so a new drawing and a new
	 * indicator plot look consistent on a fresh chart. */
	const DEFAULT_DRAWING_STYLE = { color: '#f2f2ef', width: 2, lineStyle: 'SOLID' as const };

	let {
		instrument,
		spec,
		tool,
		replayIdx,
		clearToken,
		overlayLayers,
		drawings,
		selectedDrawingId,
		onCrosshair,
		onCrosshairTime,
		crosshairTime,
		onNarrow,
		onLastClose,
		onLiveNotice,
		onPriceScale,
		onLivePrice,
		onChartApi,
		onCreateDrawing,
		onSelectDrawing,
		onToolConsumed,
		onContextMenu,
		reloadToken = 0,
		resetToken = 0,
		settings
	}: {
		instrument: string;
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
		/** The bar the crosshair sits on, so sibling panes can draw the same
		 * vertical line. `null` when the pointer leaves this pane. */
		onCrosshairTime?: (time: UTCTimestamp | null) => void;
		/** A crosshair time coming *from* a sibling pane. */
		crosshairTime?: UTCTimestamp | null;
		/** Fires whenever the container crosses the 330px width the reference
		 * uses to hide the venue prefix (paneNarrow, line 1794). */
		onNarrow?: (narrow: boolean) => void;
		/** Fires with the most recent bar's real decimal close, whenever bars
		 * (re)load — the order ticket's `mid` price comes from here now,
		 * instead of a mock candle series. */
		onLastClose?: (price: number) => void;
		onLiveNotice?: (notice: string | null) => void;
		/** Fires with `price_scale` whenever bars (re)load — sub-pane charts
		 * for this pane's Atr/Macd layers need it too (see
		 * `sub-pane-chart.svelte`'s own doc on why a sub-pane still cares
		 * about price scale). */
		onPriceScale?: (scale: number) => void;
		/** Fires with the live last-traded price, or `null`
		 * once this pane's instrument changes and no tick has arrived yet
		 * for the new one. `pane-header.svelte` shows this next to the
		 * instrument chip whenever the crosshair isn't overriding it. */
		onLivePrice?: (price: number | null) => void;
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
	let priceBadge: LastPriceBadgePrimitive | undefined;
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

	let bars = $state<{ ts_open: number; open: number; high: number; low: number; close: number }[]>([]);
	let priceScale = $state(0);
	let qtyScale = $state(0);
	// The live last-traded price, fed by `wsEventsStore` — see
	// the subscribe/tick effects below.
	let livePrice = $state<number | null>(null);
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
	function provisionalForIndicators() {
		if (forming == null) return undefined;
		const priceFactor = 10 ** priceScale;
		const qtyFactor = 10 ** qtyScale;
		return {
			ts_open: forming.ts_open,
			open: Math.round(forming.open * priceFactor),
			high: Math.round(forming.high * priceFactor),
			low: Math.round(forming.low * priceFactor),
			close: Math.round(forming.close * priceFactor),
			volume: Math.round(forming.volume * qtyFactor)
		};
	}
	// Set once this pane's subscribed topic gets back an explicit
	// `unsupported` WS frame rather than a `price` tick — the authoritative
	// half of `liveState` below; `hasLiveFeed` (the `GET /api/sources`
	// cache) is the other, checked before a subscribe even goes out.
	let unsupported = $state(false);
	let progress = $state<BarLoadProgress>({ phase: 'checking' });
	const loadGeneration = new GenerationGuard();
	// The indicator effect's own generation guard — `loadGeneration`
	// only covers bars, and the overlay reconciliation below races
	// independently of it: a layer hidden/shown/edited begins a new
	// generation here without touching `loadGeneration` at all, so a stale,
	// still-resolving indicator fetch can be told apart from the current one.
	const indicatorGeneration = new GenerationGuard();
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
			layout: { background: { color: T.bg }, textColor: T.axis, fontFamily: "'IBM Plex Mono', monospace", fontSize: 9 },
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
			layout: { background: { color: T.bg }, textColor: T.axis, fontFamily: "'IBM Plex Mono', monospace", fontSize: 9 },
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
		series = chart.addCandlestickSeries({
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
		drawingsPrimitive = new DrawingsPrimitive();
		series.attachPrimitive(drawingsPrimitive);
		priceBadge = new LastPriceBadgePrimitive();
		series.attachPrimitive(priceBadge);
		priceBadge.setColors(badgeColors(T));

		chart.subscribeCrosshairMove((param) => {
			onCrosshairTime?.((param.time as UTCTimestamp | undefined) ?? null);
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

		chart.subscribeClick((param) => {
			if (!param.point || !chart || !series) return;

			if (tool === 'cursor') {
				const hit = typeof param.hoveredObjectId === 'string' ? param.hoveredObjectId : null;
				onSelectDrawing?.(hit);
				return;
			}

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
			drawingsPrimitive = undefined;
			// Belt-and-braces alongside whatever `series`'s own disposal already
			// calls: stops the primitive's own one-second timer even if the
			// library's primitive-detach timing ever changes.
			priceBadge?.detached();
			priceBadge = undefined;
			// Fired before `chart.remove()` so a caller unsubscribing from this
			// chart's time scale (`$lib/charts/axis-sync.ts`) never touches it
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
		chart.applyOptions({ timeScale: { timeVisible: intraday, secondsVisible: false } });
	});

	// A sibling pane's crosshair, mirrored here so one pointer draws one
	// vertical line across every pane — the reading a trader expects when a
	// sub-pane sits under the price.
	//
	// The price is deliberately off-canvas: `setCrosshairPosition` needs one,
	// but only the *vertical* line belongs on a pane the pointer is not in.
	$effect(() => {
		if (!chart || !series) return;
		if (crosshairTime == null) {
			chart.clearCrosshairPosition();
			return;
		}
		const offscreen = series.coordinateToPrice(-40);
		if (offscreen == null) return;
		chart.setCrosshairPosition(offscreen, crosshairTime, series);
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
			forming = null;
		}
		const { from, to } = defaultBarWindow(currentSpec);
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

		series.priceScale().applyOptions({
			scaleMargins: { top: cs.marginTop / 100, bottom: cs.marginBottom / 100 }
		});
		chart.timeScale().applyOptions({ rightOffset: cs.marginRight });

		// How the price axis reads: linear, logarithmic, or one of the two
		// relative modes, and whether it refits itself. `autoScale` is a
		// stored setting rather than a transient because dragging the axis
		// turns it off and it stays off — a chart can otherwise be left in a
		// state no amount of panning recovers.
		const MODES = { REGULAR: 0, LOGARITHMIC: 1, PERCENT: 2, INDEXED: 3 } as const;
		series.priceScale().applyOptions({
			mode: MODES[cs.priceScaleMode] ?? 0,
			invertScale: cs.invertScale,
			autoScale: cs.autoScale
		});

		// Which side the price scale lives on.
		chart.applyOptions({
			rightPriceScale: { visible: cs.placement === 'RIGHT' || cs.placement === 'BOTH' },
			leftPriceScale: { visible: cs.placement === 'LEFT' || cs.placement === 'BOTH' }
		});

		// `VALUE + LINE` shows both the label and the line across the plot;
		// `VALUE` drops the line; `HIDDEN` drops both. The label itself is the
		// badge this pane draws, so it is switched there.
		series.applyOptions({ priceLineVisible: cs.symbolLabel === 'VALUE + LINE' });

		chart.applyOptions({
			watermark: {
				visible: cs.watermark === 'VISIBLE',
				text: instrument.toUpperCase(),
				color: pick(cs.watermarkColor, T.grid),
				fontSize: 44
			}
		});
	});

	// Time-axis labels: 24h or 12h, with or without the weekday. The library
	// formats ticks itself unless given a formatter, and its default is the
	// 24h one with no weekday.
	$effect(() => {
		if (!chart) return;
		const cs = paneSettings;
		const intraday = isIntradaySpec(spec);
		chart.applyOptions({
			timeScale: {
				tickMarkFormatter: (time: number) => {
					const at = new Date(time * 1000);
					if (!intraday) {
						const date = at.toLocaleDateString(undefined, { day: 'numeric', month: 'short' });
						return cs.dayOfWeek
							? `${at.toLocaleDateString(undefined, { weekday: 'short' })} ${date}`
							: date;
					}
					const clock = at.toLocaleTimeString(undefined, {
						hour: '2-digit',
						minute: '2-digit',
						hour12: cs.timeFormat === '12H'
					});
					return cs.dayOfWeek
						? `${at.toLocaleDateString(undefined, { weekday: 'short' })} ${clock}`
						: clock;
				}
			}
		});
	});

	/** The high/low guides, kept so they can be removed when the setting is
	 * turned off or the data changes. */
	let highLowLines: ReturnType<NonNullable<typeof series>['createPriceLine']>[] = [];

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
	}

	// Reset on request. `0` is the never-asked value, so a pane that has not
	// been asked is left exactly as the user arranged it.
	$effect(() => {
		if (resetToken > 0) resetView();
	});

	/** The series the viewport was last framed for. Panning or zooming is the
	 * user's own choice and must survive a live tick or a replay step, so the
	 * frame is only reset when the pane starts showing a *different* series. */
	let framedFor = '';

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
		if (bars.length === 0 || framedFor === identity) return;
		framedFor = identity;
		// Dragging the price axis turns `autoScale` off, and it stays off:
		// without this, moving from an instrument priced in the tens of
		// thousands to one priced in the thousands leaves the new candles
		// off-screen entirely, under the old instrument's range.
		series.priceScale().applyOptions({ autoScale: true });
		chart.timeScale().fitContent();
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
		const token = indicatorGeneration.begin();
		const layers = overlayLayers.filter((l) => l.kind === 'indicator_overlay' && l.indicatorName);
		const { from, to } = defaultBarWindow(currentSpec);
		const cut = replayIdx == null ? Infinity : replayIdx;

		// The forming bar, so an indicator's newest point lands on the same
		// candle the chart is drawing instead of stopping one bar behind it.
		// It carries volume as well as prices, so a VWAP or a volume
		// histogram follows the live bar like everything else.
		void formingVersion;
		const provisional = provisionalForIndicators();

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
						const scaleKind = indicatorFieldScale(layer.indicatorName as string, plot.field);
						const divisor = scaleKind === 'price' ? 10 ** currentPriceScale : scaleKind === 'qty' ? 10 ** currentQtyScale : 1;
						let handle = overlaySeries.get(key);
						if (!handle) {
							handle =
								plot.type === 'histogram'
									? chart!.addHistogramSeries({
											priceScaleId: 'senken-volume-overlay',
											priceFormat: { type: 'volume' },
											// The volume overlay shares the price axis but is not a
											// price: its own last-value label would sit among the
											// price ticks reading something like `3.02K`.
											lastValueVisible: false,
											priceLineVisible: false
										})
									: chart!.addLineSeries({ priceLineVisible: false, lastValueVisible: false });
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
	const liveState = $derived(deriveLiveState(hasLiveFeed(instrument), unsupported));

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
</script>

<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
	bind:this={container}
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
