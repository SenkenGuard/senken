// Renders and hit-tests one pane's drawing objects directly on the
// candlestick series' own canvas, via `lightweight-charts`' series-primitive
// API (`ISeriesPrimitive`, attached with `series.attachPrimitive`) — the
// same mechanism the library's own plugin examples use to draw shapes a
// built-in series type cannot. A horizontal line, trend line and rectangle
// all go through this one primitive rather than three separate ones, since
// they share the same pixel-space drawing code and the same hit-test entry
// point the chart calls on every mouse move.
//
// This exists because a `lightweight-charts` `IPriceLine` (what the old
// click-to-paint toolbar used) has no identity a click can resolve back to
// and no way to draw a diagonal line or a filled zone at all — an object
// model needs a renderer that knows about every drawing at once, not one
// price line per click.
//
// Selecting a drawing puts it in move mode (TradingView's own behaviour)
// rather than opening a properties dialog: `hitTestDetailed`/`beginDrag`/
// `updateDrag`/`endDrag` below are what the chart pane drives on
// mousedown/mousemove/mouseup once a drawing is selected. The actual
// hit-test and drag geometry is `./drawing-hit.ts` — kept pure so it is
// testable without a chart at all; this class only ever converts between a
// `DrawingRuntime`'s (time, price) fields and the pixel/domain points that
// module works with.
import type {
	IChartApi,
	ISeriesApi,
	ISeriesPrimitive,
	IPrimitivePaneRenderer,
	IPrimitivePaneView,
	Logical,
	PrimitiveHoveredItem,
	SeriesAttachedParameter,
	Time
} from 'lightweight-charts';
import type { CanvasRenderingTarget2D } from 'fancy-canvas';
import type { DrawingRuntime } from './pane-runtime';
import { handlePoints, hitTestDrawing, moveHandle, translateGeometry, type DrawingGeometry, type DrawingHit, type Point } from './drawing-hit';

/** Radius of a selected drawing's own handle dot, in pixels — smaller than
 * `HANDLE_HIT_RADIUS_PX` (`drawing-hit.ts`) so the dot looks small while
 * still being easy to grab. */
const HANDLE_RADIUS_PX = 4;

/** A drag in progress: which drawing, which part of it was grabbed, and the
 * pointer's last known position in that drawing's own (time, price) domain
 * — a body drag needs the previous position to compute a delta; a handle
 * drag only ever needs the current one, but tracking it uniformly keeps
 * `updateDrag` a single code path. */
interface ActiveDrag {
	id: string;
	hit: DrawingHit;
	lastDomainPoint: Point;
}

class DrawingsRenderer implements IPrimitivePaneRenderer {
	constructor(private readonly primitive: DrawingsPrimitive) {}

	draw(target: CanvasRenderingTarget2D): void {
		target.useMediaCoordinateSpace(({ context, mediaSize }) => {
			this.primitive.paint(context, mediaSize.width, mediaSize.height);
		});
	}
}

class DrawingsPaneView implements IPrimitivePaneView {
	constructor(private readonly primitive: DrawingsPrimitive) {}

	renderer(): IPrimitivePaneRenderer {
		return new DrawingsRenderer(this.primitive);
	}
}

/** One pane's drawing objects, attached to that pane's candlestick series. */
export class DrawingsPrimitive implements ISeriesPrimitive<Time> {
	private chart: IChartApi | undefined;
	private series: ISeriesApi<'Candlestick'> | undefined;
	private requestUpdate: (() => void) | undefined;
	private readonly view = new DrawingsPaneView(this);
	private drawings: DrawingRuntime[] = [];
	private selectedId: string | null = null;
	/** A trend line/rectangle's second point while it is still being drawn —
	 * rendered as a live preview, never persisted. */
	private pendingStart: { time: number; price: number } | null = null;
	private pendingEnd: { time: number; price: number } | null = null;
	/** The pane's own bar grid (first loaded bar's time, its step) — needed
	 * to place a point in time. `chart.timeScale().timeToCoordinate(time)`
	 * only finds a coordinate for a time that exactly matches an already-
	 * plotted bar ("or `null` if no time found on time scale", per its own
	 * doc); a trend line/rectangle corner dropped into the empty margin past
	 * the last bar has no such match. Reconstructing a logical index from the
	 * grid and going through `logicalToCoordinate` instead (documented to
	 * return `null` only when the chart has no data at all) renders a point
	 * anywhere on the axis, on-bar or not. The same grid inverts a pixel `x`
	 * back to a time for a drag (`xToTime` below). */
	private firstBarSeconds: number | undefined;
	private stepSeconds: number | undefined;
	/** The pane's last-painted width, in pixels — a horizontal line has no
	 * `x` of its own (see `DrawingGeometry`'s doc), so its one handle is
	 * placed at the pane's horizontal center, recomputed every paint. */
	private lastWidth = 0;
	private drag: ActiveDrag | null = null;
	private onDragEndCallback: ((id: string, patch: Partial<Pick<DrawingRuntime, 'price' | 'start' | 'end'>>) => void) | undefined;

	attached({ chart, series, requestUpdate }: SeriesAttachedParameter<Time>): void {
		this.chart = chart as IChartApi;
		this.series = series as ISeriesApi<'Candlestick'>;
		this.requestUpdate = requestUpdate;
	}

	detached(): void {
		this.chart = undefined;
		this.series = undefined;
		this.requestUpdate = undefined;
	}

	/** Drawings never widen the price axis.
	 *
	 * Auto-fit is about the data: a trend line extended far above the market,
	 * or a level left at an old price, would otherwise drag the visible range
	 * out to reach it and squash the candles the reader is actually looking
	 * at. Returning `null` opts this primitive out of the calculation and
	 * leaves the range to the bars.
	 */
	autoscaleInfo(): null {
		return null;
	}

	paneViews(): readonly IPrimitivePaneView[] {
		return [this.view];
	}

	setDrawings(drawings: DrawingRuntime[]): void {
		this.drawings = drawings;
		this.requestUpdate?.();
	}

	setSelected(id: string | null): void {
		this.selectedId = id;
		this.requestUpdate?.();
	}

	/** Updates the bar grid used to place a drawing's own (time, price)
	 * points — see the field docs above. Called whenever the pane's loaded
	 * bars or timeframe change. */
	setBarGrid(firstBarSeconds: number, stepSeconds: number): void {
		this.firstBarSeconds = firstBarSeconds;
		this.stepSeconds = stepSeconds;
		this.requestUpdate?.();
	}

	/** Shows a live preview of a trend line/rectangle's second point while
	 * the user is still drawing it (after the first click, before the
	 * second) — `null`/`null` clears it. */
	setPending(start: { time: number; price: number } | null, end: { time: number; price: number } | null): void {
		this.pendingStart = start;
		this.pendingEnd = end;
		this.requestUpdate?.();
	}

	/** Registers the callback fired once a drag completes (`endDrag`) with
	 * the dragged drawing's id and its new geometry, so the chart pane can
	 * persist it. Fires only at drag end, never per-frame — this class
	 * already updates its own `drawings` copy live for visual feedback while
	 * the drag is in progress. */
	setOnDragEnd(callback: (id: string, patch: Partial<Pick<DrawingRuntime, 'price' | 'start' | 'end'>>) => void): void {
		this.onDragEndCallback = callback;
	}

	/** Whether a handle/body drag is currently in progress. */
	isDragging(): boolean {
		return this.drag != null;
	}

	/** The chart pane's move-mode entry point — call on mousedown/click at
	 * `(x, y)` (pixels, the same space `hitTest` below uses) to find what to
	 * start dragging. Only the *selected* drawing's handles are ever
	 * reported: an unselected drawing shows no handles, so landing near
	 * where one would be still counts as a body hit (select-and-drag, or
	 * just select). */
	hitTestDetailed(x: number, y: number): { id: string; hit: DrawingHit } | null {
		if (this.selectedId != null) {
			const selected = this.drawings.find((d) => d.id === this.selectedId);
			const geometry = selected ? this.pixelGeometryFor(selected) : null;
			const hit = geometry ? hitTestDrawing(geometry, { x, y }) : null;
			if (selected && hit?.part === 'handle') return { id: selected.id, hit };
		}
		for (let i = this.drawings.length - 1; i >= 0; i--) {
			const d = this.drawings[i];
			const geometry = this.pixelGeometryFor(d);
			if (geometry && hitTestDrawing(geometry, { x, y })) {
				return { id: d.id, hit: { part: 'body' } };
			}
		}
		return null;
	}

	/** Starts a drag on `id`'s `hit` (from `hitTestDetailed`), seeded at
	 * pixel position `(x, y)`. */
	beginDrag(id: string, hit: DrawingHit, x: number, y: number): void {
		const domainPoint = this.toDomainPoint(x, y);
		if (!domainPoint) return;
		this.drag = { id, hit, lastDomainPoint: domainPoint };
	}

	/** Moves the drag in progress to pixel position `(x, y)` — a no-op if
	 * `beginDrag` was never called or the dragged drawing has since been
	 * removed. Updates this primitive's own copy of the drawing immediately,
	 * so the move renders live; the chart pane only learns the final result
	 * once `endDrag` fires its callback. */
	updateDrag(x: number, y: number): void {
		if (!this.drag) return;
		const drawing = this.drawings.find((d) => d.id === this.drag?.id);
		if (!drawing) {
			this.drag = null;
			return;
		}
		const domainPoint = this.toDomainPoint(x, y);
		if (!domainPoint) return;
		const current = this.domainGeometryFor(drawing);
		const moved =
			this.drag.hit.part === 'handle'
				? moveHandle(current, this.drag.hit.handle, domainPoint)
				: translateGeometry(current, domainPoint.x - this.drag.lastDomainPoint.x, domainPoint.y - this.drag.lastDomainPoint.y);
		this.applyGeometry(drawing, moved);
		this.drag.lastDomainPoint = domainPoint;
		this.requestUpdate?.();
	}

	/** Ends the drag in progress, if any, firing `setOnDragEnd`'s callback
	 * with the dragged drawing's final geometry. */
	endDrag(): void {
		if (!this.drag) return;
		const drawing = this.drawings.find((d) => d.id === this.drag?.id);
		this.drag = null;
		if (!drawing) return;
		this.onDragEndCallback?.(drawing.id, drawing.kind === 'horizontal_line' ? { price: drawing.price } : { start: drawing.start, end: drawing.end });
	}

	private applyGeometry(drawing: DrawingRuntime, geometry: DrawingGeometry): void {
		if (geometry.kind === 'horizontal_line') {
			drawing.price = geometry.y;
		} else {
			drawing.start = { time: geometry.start.x, price: geometry.start.y };
			drawing.end = { time: geometry.end.x, price: geometry.end.y };
		}
	}

	/** `(x, y)` pixels -> this drawing's own (time, price) domain, or `null`
	 * if either axis cannot currently resolve it (no chart yet, or bars not
	 * loaded — same conditions `priceToY`/`timeToX` already guard). */
	private toDomainPoint(x: number, y: number): Point | null {
		const price = this.series?.coordinateToPrice(y);
		const time = this.xToTime(x);
		if (price == null || time == null) return null;
		return { x: time, y: price };
	}

	/** The inverse of `timeToX` — see that method's doc for why this falls
	 * back to the bar grid instead of only `coordinateToTime`. */
	private xToTime(x: number): number | null {
		if (!this.chart) return null;
		const t = this.chart.timeScale().coordinateToTime(x);
		if (t != null) return Number(t);
		if (this.firstBarSeconds == null || !this.stepSeconds) return null;
		const logical = this.chart.timeScale().coordinateToLogical(x);
		if (logical == null) return null;
		return this.firstBarSeconds + Math.round(logical) * this.stepSeconds;
	}

	private priceToY(price: number): number | null {
		const y = this.series?.priceToCoordinate(price);
		return y == null ? null : y;
	}

	private timeToX(time: number): number | null {
		if (!this.chart || this.firstBarSeconds == null || !this.stepSeconds) return null;
		const logical = (time - this.firstBarSeconds) / this.stepSeconds;
		const x = this.chart.timeScale().logicalToCoordinate(logical as Logical);
		return x == null ? null : x;
	}

	/** `d`'s geometry in screen pixels, for painting and for hit-testing —
	 * `null` if any needed coordinate cannot currently be resolved (mirrors
	 * the guards `paintOne`/`hits` always had). */
	private pixelGeometryFor(d: DrawingRuntime): DrawingGeometry | null {
		if (d.kind === 'horizontal_line') {
			const y = this.priceToY(d.price ?? 0);
			return y == null ? null : { kind: 'horizontal_line', y, handleX: this.lastWidth / 2 };
		}
		if (!d.start || !d.end) return null;
		const x1 = this.timeToX(d.start.time);
		const y1 = this.priceToY(d.start.price);
		const x2 = this.timeToX(d.end.time);
		const y2 = this.priceToY(d.end.price);
		if (x1 == null || y1 == null || x2 == null || y2 == null) return null;
		return { kind: d.kind, start: { x: x1, y: y1 }, end: { x: x2, y: y2 } };
	}

	/** `d`'s geometry in its own (time, price) domain — used only for drag
	 * maths (`updateDrag`), never for painting. `handleX` has no domain
	 * meaning (a horizontal line has no time of its own); it is never read
	 * by `moveHandle`/`translateGeometry` for this kind, so `0` is a plain
	 * placeholder, not a real value. */
	private domainGeometryFor(d: DrawingRuntime): DrawingGeometry {
		if (d.kind === 'horizontal_line') {
			return { kind: 'horizontal_line', y: d.price ?? 0, handleX: 0 };
		}
		return {
			kind: d.kind,
			start: { x: d.start?.time ?? 0, y: d.start?.price ?? 0 },
			end: { x: d.end?.time ?? 0, y: d.end?.price ?? 0 }
		};
	}

	paint(ctx: CanvasRenderingContext2D, width: number, height: number): void {
		if (!this.chart || !this.series) return;
		this.lastWidth = width;
		for (const d of this.drawings) {
			this.paintOne(ctx, width, d, d.id === this.selectedId);
		}
		if (this.pendingStart && this.pendingEnd) {
			this.paintSegmentPreview(ctx, this.pendingStart, this.pendingEnd);
		}
		void height;
	}

	private setStrokeStyle(ctx: CanvasRenderingContext2D, color: string, width: number, style: 'SOLID' | 'DASHED' | 'DOTTED', selected: boolean): void {
		ctx.strokeStyle = color;
		ctx.lineWidth = selected ? width + 1.5 : width;
		ctx.setLineDash(style === 'DASHED' ? [6, 4] : style === 'DOTTED' ? [1.5, 3] : []);
	}

	private paintOne(ctx: CanvasRenderingContext2D, width: number, d: DrawingRuntime, selected: boolean): void {
		const geometry = this.pixelGeometryFor(d);
		if (!geometry) return;

		ctx.save();
		this.setStrokeStyle(ctx, d.color, d.width, d.lineStyle, selected);
		if (geometry.kind === 'horizontal_line') {
			ctx.beginPath();
			ctx.moveTo(0, geometry.y);
			ctx.lineTo(width, geometry.y);
			ctx.stroke();
		} else if (geometry.kind === 'trend_line') {
			ctx.beginPath();
			ctx.moveTo(geometry.start.x, geometry.start.y);
			ctx.lineTo(geometry.end.x, geometry.end.y);
			ctx.stroke();
		} else {
			const left = Math.min(geometry.start.x, geometry.end.x);
			const top = Math.min(geometry.start.y, geometry.end.y);
			const w = Math.abs(geometry.end.x - geometry.start.x);
			const h = Math.abs(geometry.end.y - geometry.start.y);
			ctx.fillStyle = withAlpha(d.color, 0.12);
			ctx.fillRect(left, top, w, h);
			ctx.strokeRect(left, top, w, h);
		}
		ctx.restore();

		if (selected) this.paintHandles(ctx, geometry, d.color);
	}

	/** Small circles at a selected drawing's handles — one for a horizontal
	 * line's single (vertical-only) handle, two for a trend line/rectangle's
	 * endpoints. Positions come from `handlePoints` so painting and
	 * hit-testing can never disagree about where a handle is. */
	private paintHandles(ctx: CanvasRenderingContext2D, geometry: DrawingGeometry, color: string): void {
		ctx.save();
		ctx.fillStyle = color;
		for (const { point } of handlePoints(geometry)) {
			ctx.beginPath();
			ctx.arc(point.x, point.y, HANDLE_RADIUS_PX, 0, Math.PI * 2);
			ctx.fill();
		}
		ctx.restore();
	}

	private paintSegmentPreview(ctx: CanvasRenderingContext2D, start: { time: number; price: number }, end: { time: number; price: number }): void {
		const x1 = this.timeToX(start.time);
		const y1 = this.priceToY(start.price);
		const x2 = this.timeToX(end.time);
		const y2 = this.priceToY(end.price);
		if (x1 == null || y1 == null || x2 == null || y2 == null) return;
		ctx.save();
		ctx.strokeStyle = 'rgba(255,255,255,0.55)';
		ctx.lineWidth = 1;
		ctx.setLineDash([4, 4]);
		ctx.beginPath();
		ctx.moveTo(x1, y1);
		ctx.lineTo(x2, y2);
		ctx.stroke();
		ctx.restore();
	}

	/** Called by the chart on every mouse move/click — returns the topmost
	 * drawing under `(x, y)`, or `null`. The chart surfaces a hit's
	 * `externalId` back on `MouseEventParams.hoveredObjectId`. Reuses the
	 * same hit-test move mode drives (`hitTestDetailed`), so a click that
	 * selects a drawing and a click that finds its handle never disagree
	 * about what counts as "on" it. */
	hitTest(x: number, y: number): PrimitiveHoveredItem | null {
		if (!this.chart || !this.series) return null;
		const hit = this.hitTestDetailed(x, y);
		return hit ? { externalId: hit.id, zOrder: 'normal' } : null;
	}
}

/** `#rrggbb` -> `rgba(r,g,b,alpha)`, for a rectangle's translucent fill —
 * the stored colour is always plain hex (`senken_workspace`'s own
 * validation), so this never has to handle any other input shape. */
function withAlpha(hex: string, alpha: number): string {
	const r = Number.parseInt(hex.slice(1, 3), 16);
	const g = Number.parseInt(hex.slice(3, 5), 16);
	const b = Number.parseInt(hex.slice(5, 7), 16);
	return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}
