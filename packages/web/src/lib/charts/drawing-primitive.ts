// Renders and hit-tests one pane's drawable objects directly on the
// candlestick series' own canvas, via `lightweight-charts`' series-primitive
// API (`ISeriesPrimitive`, attached with `series.attachPrimitive`) — the
// same mechanism the library's own plugin examples use to draw shapes a
// built-in series type cannot.
//
// This is the **single** renderer for two sources that produce the exact
// same `ObjectDrawable` shape (`./drawable.ts`): a pane's persisted
// drawings (`setDrawings`, converted from `DrawingRuntime` via
// `drawableFromDrawingRuntime`) and an indicator's own non-series display
// output (`setIndicatorObjects`). Both end up in `allObjects()` and are
// painted/hit-tested by the exact same code below — a level, a segment, a
// box and a label all go through this one primitive rather than a second
// one per source, since they share the same pixel-space drawing code and
// the same hit-test entry point the chart calls on every mouse move,
// regardless of which side produced the geometry.
//
// This exists because a `lightweight-charts` `IPriceLine` (what the old
// click-to-paint toolbar used) has no identity a click can resolve back to
// and no way to draw a diagonal line, a filled zone or a label at all — an
// object model needs a renderer that knows about every drawable at once,
// not one price line per click.
//
// Selecting a drawable puts it in move mode (TradingView's own behaviour)
// rather than opening a properties dialog: `hitTestDetailed`/`beginDrag`/
// `updateDrag`/`endDrag` below are what the chart pane drives on
// mousedown/mousemove/mouseup once a drawable is selected — only a
// *persisted* one (`this.persisted`) can actually be dragged; an
// indicator's own object is display-only and `beginDrag` refuses to start
// a drag on one, the same way it always refused to drag anything that
// was not found. The actual hit-test and drag geometry is
// `./drawing-hit.ts`, kept pure so it is testable without a chart at all;
// this class only ever converts between a pane's own (time, price) domain
// and the pixel points that module works with.
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
import { DEFAULT_LABEL_STYLE, drawablesFromDrawingRuntime, patchFromDrawable, type DrawablePoint, type LabelStyle, type ObjectDrawable } from './drawable';
import {
	extendSegment,
	handlePoints,
	hitTestDrawable,
	labelBoxRect,
	labelBoxSize,
	moveHandle,
	translateGeometry,
	type DrawableGeometry,
	type DrawingHit
} from './drawing-hit';

/** Radius of a selected drawable's own handle dot, in pixels — smaller than
 * `HANDLE_HIT_RADIUS_PX` (`drawing-hit.ts`) so the dot looks small while
 * still being easy to grab. */
const HANDLE_RADIUS_PX = 4;

/** A drag in progress: which drawable, which part of it was grabbed, and the
 * pointer's last known position in that drawable's own (time, price) domain
 * — a body drag needs the previous position to compute a delta; a handle
 * drag only ever needs the current one, but tracking it uniformly keeps
 * `updateDrag` a single code path. */
interface ActiveDrag {
	id: string;
	hit: DrawingHit;
	lastDomainPoint: DrawablePoint;
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

/** One pane's drawable objects, attached to that pane's candlestick series. */
export class DrawingsPrimitive implements ISeriesPrimitive<Time> {
	private chart: IChartApi | undefined;
	private series: ISeriesApi<'Candlestick'> | undefined;
	private requestUpdate: (() => void) | undefined;
	private readonly view = new DrawingsPaneView(this);
	/** This pane's real, persisted drawings — the only objects a drag may
	 * ever move (`beginDrag` below). */
	private persisted: ObjectDrawable[] = [];
	/** Non-series objects an indicator's own display list emitted for this
	 * pane, tagged with the owning layer's id — display-only, never
	 * draggable. Empty today: `POST /api/indicators/compute`'s response
	 * carries only `series` drawables until `crates/api/src/dto/indicators.rs`
	 * widens `IndicatorDrawableDto` to the rest of `senken_indicators::Drawable`
	 * The field and `setIndicatorObjects` exist
	 * now so wiring that response through costs one call site, not a second
	 * renderer. */
	private indicatorObjects: ObjectDrawable[] = [];
	private selectedId: string | null = null;
	/** A trend line/rectangle's second point while it is still being drawn —
	 * rendered as a live preview, never persisted. */
	private pendingStart: { time: number; price: number } | null = null;
	private pendingEnd: { time: number; price: number } | null = null;
	/** The measure tool's live readout — a plain text box anchored at one
	 * point, painted with `paintTextBox` (the same box/text painting
	 * `paintLabel` uses for a `Label` drawable) but never added to
	 * `persisted`/`indicatorObjects`: a measurement is never selected,
	 * dragged or hit-tested, only shown and replaced. */
	private measureAt: { time: number; price: number } | null = null;
	private measureText: string | null = null;
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
	/** The pane's last-painted size, in pixels — a level has no `x` of its
	 * own (see `DrawableGeometry`'s doc), so its one handle is placed at the
	 * pane's horizontal center, recomputed every paint; a segment's extended
	 * draw endpoints (`extendSegment`) need both dimensions to reach past
	 * whichever edge they extend toward. */
	private lastWidth = 0;
	private lastHeight = 0;
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

	/** Drawables never widen the price axis.
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

	/** Replaces this pane's persisted drawings, converted to the shared
	 * `ObjectDrawable` shape (`drawablesFromDrawingRuntime`) — the only place
	 * a `DrawingRuntime` is read at all past this call. `flatMap`, not `map`:
	 * a `fib_retracement` drawing fans out to six `Level`s (see that
	 * function's own doc); every other kind still contributes exactly one.
	 *
	 * A drawing whose `visible` is `false` is dropped here rather than
	 * carried through and hidden at paint time — the object tree's eye
	 * toggle must mean the object is actually gone from the canvas (and from
	 * hit-testing/dragging), not merely invisible while still clickable.
	 * `drawings`' own order is kept as given: the caller passes them in
	 * `position` order, and later entries are painted, and hit-tested, on
	 * top of earlier ones (see `hitTestDetailed`'s reverse walk). */
	setDrawings(drawings: DrawingRuntime[]): void {
		this.persisted = drawings.filter((d) => d.visible).flatMap(drawablesFromDrawingRuntime);
		this.requestUpdate?.();
	}

	/** Replaces the non-series objects an indicator's display list emitted
	 * for this pane. See the field doc on `indicatorObjects` for why nothing
	 * calls this yet. */
	setIndicatorObjects(objects: ObjectDrawable[]): void {
		this.indicatorObjects = objects;
		this.requestUpdate?.();
	}

	private allObjects(): ObjectDrawable[] {
		return [...this.persisted, ...this.indicatorObjects];
	}

	setSelected(id: string | null): void {
		this.selectedId = id;
		this.requestUpdate?.();
	}

	/** Updates the bar grid used to place a drawable's own (time, price)
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

	/** Shows (or, called with `null`/`null`, clears) the measure tool's
	 * readout at `at` — the chart pane computes `text` itself
	 * (`$lib/charts/measure.ts`'s `formatMeasureLabel`); this primitive only
	 * paints whatever string it is given. */
	setMeasurePreview(at: { time: number; price: number } | null, text: string | null): void {
		this.measureAt = at;
		this.measureText = text;
		this.requestUpdate?.();
	}

	/** Registers the callback fired once a drag completes (`endDrag`) with
	 * the dragged drawing's id and its new geometry, so the chart pane can
	 * persist it. Fires only at drag end, never per-frame — this class
	 * already updates its own copy live for visual feedback while the drag
	 * is in progress. */
	setOnDragEnd(callback: (id: string, patch: Partial<Pick<DrawingRuntime, 'price' | 'start' | 'end'>>) => void): void {
		this.onDragEndCallback = callback;
	}

	/** Whether a handle/body drag is currently in progress. */
	isDragging(): boolean {
		return this.drag != null;
	}

	/** The chart pane's move-mode entry point — call on mousedown/click at
	 * `(x, y)` (pixels, the same space `hitTest` below uses) to find what to
	 * start dragging. Only the *selected* drawable's handles are ever
	 * reported: an unselected drawable shows no handles, so landing near
	 * where one would be still counts as a body hit (select-and-drag, or
	 * just select). Searches persisted and indicator-emitted objects alike —
	 * an indicator's own `Box`/`Label` is just as hit-testable as a
	 * persisted one, which is what makes "clicking an indicator's object
	 * selects the indicator" true with no extra code. */
	hitTestDetailed(x: number, y: number): { id: string; hit: DrawingHit } | null {
		const objects = this.allObjects();
		if (this.selectedId != null) {
			const selected = objects.find((d) => d.ownerId === this.selectedId);
			const geometry = selected ? this.pixelGeometryFor(selected) : null;
			const hit = geometry ? hitTestDrawable(geometry, { x, y }) : null;
			if (selected && hit?.part === 'handle') return { id: selected.ownerId, hit };
		}
		for (let i = objects.length - 1; i >= 0; i--) {
			const d = objects[i];
			const geometry = this.pixelGeometryFor(d);
			if (geometry && hitTestDrawable(geometry, { x, y })) {
				return { id: d.ownerId, hit: { part: 'body' } };
			}
		}
		return null;
	}

	/** Starts a drag on `id`'s `hit` (from `hitTestDetailed`), seeded at
	 * pixel position `(x, y)` — a no-op if `id` is not one of this pane's
	 * *persisted* drawings: an indicator's own object can be selected (see
	 * `hitTestDetailed`) but never dragged, since there is nothing to write
	 * a moved position back to. */
	beginDrag(id: string, hit: DrawingHit, x: number, y: number): void {
		if (!this.persisted.some((d) => d.ownerId === id)) return;
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
		const index = this.persisted.findIndex((d) => d.ownerId === this.drag?.id);
		if (index === -1) {
			this.drag = null;
			return;
		}
		const domainPoint = this.toDomainPoint(x, y);
		if (!domainPoint) return;
		const current = this.persisted[index];
		const moved =
			this.drag.hit.part === 'handle'
				? moveHandle(current, this.drag.hit.handle, domainPoint)
				: translateGeometry(current, domainPoint.time - this.drag.lastDomainPoint.time, domainPoint.value - this.drag.lastDomainPoint.value);
		this.persisted[index] = moved;
		this.drag.lastDomainPoint = domainPoint;
		this.requestUpdate?.();
	}

	/** Ends the drag in progress, if any, firing `setOnDragEnd`'s callback
	 * with the dragged drawing's final geometry. */
	endDrag(): void {
		if (!this.drag) return;
		const drawable = this.persisted.find((d) => d.ownerId === this.drag?.id);
		this.drag = null;
		if (!drawable) return;
		this.onDragEndCallback?.(drawable.ownerId, patchFromDrawable(drawable));
	}

	/** `(x, y)` pixels -> this pane's own (time, price) domain, or `null` if
	 * either axis cannot currently resolve it (no chart yet, or bars not
	 * loaded — same conditions `priceToY`/`timeToX` already guard). */
	private toDomainPoint(x: number, y: number): DrawablePoint | null {
		const price = this.series?.coordinateToPrice(y);
		const time = this.xToTime(x);
		if (price == null || time == null) return null;
		return { time, value: price };
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
	 * `null` if any needed coordinate cannot currently be resolved. This is
	 * the one place a drawable's domain (time, price) anchors become pixel
	 * positions; `hitTestDetailed` and `paintOne` both call it, so a click
	 * and the shape it is testing against can never disagree about where
	 * that shape actually is. */
	private pixelGeometryFor(d: ObjectDrawable): DrawableGeometry | null {
		if (d.kind === 'level') {
			const y = this.priceToY(d.price);
			return y == null ? null : { kind: 'level', y, handleX: this.lastWidth / 2 };
		}
		if (d.kind === 'label') {
			const x = this.timeToX(d.at.time);
			const y = this.priceToY(d.at.value);
			if (x == null || y == null) return null;
			const size = labelBoxSize(d.text, d.style.fontSize);
			return { kind: 'label', at: { x, y }, box: labelBoxRect({ x, y }, size, d.anchor) };
		}
		const x1 = this.timeToX(d.a.time);
		const y1 = this.priceToY(d.a.value);
		const x2 = this.timeToX(d.b.time);
		const y2 = this.priceToY(d.b.value);
		if (x1 == null || y1 == null || x2 == null || y2 == null) return null;
		if (d.kind === 'box') {
			return { kind: 'box', start: { x: x1, y: y1 }, end: { x: x2, y: y2 } };
		}
		const { start: drawStart, end: drawEnd } = extendSegment(
			{ x: x1, y: y1 },
			{ x: x2, y: y2 },
			d.extend,
			this.lastWidth,
			this.lastHeight
		);
		return { kind: 'segment', anchorStart: { x: x1, y: y1 }, anchorEnd: { x: x2, y: y2 }, drawStart, drawEnd };
	}

	paint(ctx: CanvasRenderingContext2D, width: number, height: number): void {
		if (!this.chart || !this.series) return;
		this.lastWidth = width;
		this.lastHeight = height;
		for (const d of this.allObjects()) {
			this.paintOne(ctx, width, d, d.ownerId === this.selectedId);
		}
		if (this.pendingStart && this.pendingEnd) {
			this.paintSegmentPreview(ctx, this.pendingStart, this.pendingEnd);
		}
		this.paintMeasure(ctx);
	}

	private setStrokeStyle(ctx: CanvasRenderingContext2D, color: string, width: number, style: 'SOLID' | 'DASHED' | 'DOTTED', selected: boolean): void {
		ctx.strokeStyle = color;
		ctx.lineWidth = selected ? width + 1.5 : width;
		ctx.setLineDash(style === 'DASHED' ? [6, 4] : style === 'DOTTED' ? [1.5, 3] : []);
	}

	private paintOne(ctx: CanvasRenderingContext2D, width: number, d: ObjectDrawable, selected: boolean): void {
		const geometry = this.pixelGeometryFor(d);
		if (!geometry) return;

		if (geometry.kind === 'label') {
			if (d.kind === 'label') this.paintLabel(ctx, geometry, d, selected);
			return;
		}
		if (d.kind === 'label') return; // unreachable: `geometry.kind` and `d.kind` always agree.

		ctx.save();
		this.setStrokeStyle(ctx, d.style.color, d.style.width, d.style.lineStyle, selected);
		switch (geometry.kind) {
			case 'level':
				ctx.beginPath();
				ctx.moveTo(0, geometry.y);
				ctx.lineTo(width, geometry.y);
				ctx.stroke();
				break;
			case 'segment':
				ctx.beginPath();
				ctx.moveTo(geometry.drawStart.x, geometry.drawStart.y);
				ctx.lineTo(geometry.drawEnd.x, geometry.drawEnd.y);
				ctx.stroke();
				break;
			case 'box': {
				if (d.kind !== 'box') break; // same invariant as above.
				const left = Math.min(geometry.start.x, geometry.end.x);
				const top = Math.min(geometry.start.y, geometry.end.y);
				const w = Math.abs(geometry.end.x - geometry.start.x);
				const h = Math.abs(geometry.end.y - geometry.start.y);
				ctx.fillStyle = withAlpha(d.style.color, d.style.fillOpacity);
				ctx.fillRect(left, top, w, h);
				ctx.strokeRect(left, top, w, h);
				break;
			}
		}
		ctx.restore();

		if (selected) this.paintHandles(ctx, geometry, d.style.color);
	}

	private paintLabel(
		ctx: CanvasRenderingContext2D,
		geometry: Extract<DrawableGeometry, { kind: 'label' }>,
		d: Extract<ObjectDrawable, { kind: 'label' }>,
		selected: boolean
	): void {
		this.paintTextBox(ctx, geometry.box, d.text, d.style);
		if (selected) this.paintHandles(ctx, geometry, d.style.textColor);
	}

	/** A filled background rectangle with centred text — shared by a `Label`
	 * drawable (`paintLabel`) and the measure tool's readout (`paintMeasure`),
	 * which is not a `Label` (it is never selected, dragged or persisted) but
	 * paints identically once it has a box and a style. */
	private paintTextBox(ctx: CanvasRenderingContext2D, box: { x: number; y: number; width: number; height: number }, text: string, style: LabelStyle): void {
		ctx.save();
		ctx.fillStyle = style.background;
		ctx.fillRect(box.x, box.y, box.width, box.height);
		ctx.fillStyle = style.textColor;
		ctx.font = `${style.fontSize}px sans-serif`;
		ctx.textBaseline = 'middle';
		ctx.textAlign = 'center';
		ctx.fillText(text, box.x + box.width / 2, box.y + box.height / 2);
		ctx.restore();
	}

	/** Paints the measure tool's live readout, if one is set — same box/text
	 * painting as a `Label`, anchored `above` its point the same way a label
	 * would be, but sourced from `measureAt`/`measureText` rather than
	 * `allObjects()`: see those fields' own doc for why. */
	private paintMeasure(ctx: CanvasRenderingContext2D): void {
		if (!this.measureAt || !this.measureText) return;
		const x = this.timeToX(this.measureAt.time);
		const y = this.priceToY(this.measureAt.price);
		if (x == null || y == null) return;
		const size = labelBoxSize(this.measureText, DEFAULT_LABEL_STYLE.fontSize);
		const box = labelBoxRect({ x, y }, size, 'above');
		this.paintTextBox(ctx, box, this.measureText, DEFAULT_LABEL_STYLE);
	}

	/** Small circles at a selected drawable's handles — one for a level's
	 * single (vertical-only) handle, two for a segment/box's endpoints, one
	 * for a label's anchor. Positions come from `handlePoints` so painting
	 * and hit-testing can never disagree about where a handle is. */
	private paintHandles(ctx: CanvasRenderingContext2D, geometry: DrawableGeometry, color: string): void {
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
	 * drawable under `(x, y)`, or `null`. The chart surfaces a hit's
	 * `externalId` back on `MouseEventParams.hoveredObjectId`. Reuses the
	 * same hit-test move mode drives (`hitTestDetailed`), so a click that
	 * selects a drawable and a click that finds its handle never disagree
	 * about what counts as "on" it. */
	hitTest(x: number, y: number): PrimitiveHoveredItem | null {
		if (!this.chart || !this.series) return null;
		const hit = this.hitTestDetailed(x, y);
		return hit ? { externalId: hit.id, zOrder: 'normal' } : null;
	}
}

/** `#rrggbb` -> `rgba(r,g,b,alpha)`, for a box's translucent fill — the
 * stored colour is always plain hex (`senken_workspace`'s own validation),
 * so this never has to handle any other input shape. */
function withAlpha(hex: string, alpha: number): string {
	const r = Number.parseInt(hex.slice(1, 3), 16);
	const g = Number.parseInt(hex.slice(3, 5), 16);
	const b = Number.parseInt(hex.slice(5, 7), 16);
	return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}
