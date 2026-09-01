// Pure geometry for chart drawable objects — one hit-test, one paint-time
// geometry builder, one drag-edit set of functions for every
// `ObjectDrawable` kind (`level`, `segment`, `box`, `label` — see
// `./drawable.ts`), whatever produced it: a persisted drawing
// (`horizontal_line`/`trend_line`/`rectangle`, converted by
// `drawableFromDrawingRuntime`) or an indicator's own display list. Kept
// apart from `drawing-primitive.ts` so the geometry itself is testable
// without a chart, a canvas, or `lightweight-charts` at all — exactly the
// discipline that already existed here for three kinds; this module widens
// it to all four without adding a second engine for the new ones.
//
// Two coordinate spaces meet in this module and are never confused:
// `DrawablePoint` (`./drawable.ts`) is a pane's own (time, price) domain —
// what `moveHandle`/`translateGeometry` edit while a drag is in progress —
// and `Point` below is screen pixels — what `DrawableGeometry`,
// `handlePoints` and `hitTestDrawable` work in. `drawing-primitive.ts` is
// the only place that converts between them.

import type { DrawablePoint, Extend, LabelAnchor, ObjectDrawable } from './drawable';

/** A point in screen pixels. */
export interface Point {
	x: number;
	y: number;
}

/** Pixels a pointer may be from a handle and still grab it — larger than
 * `BODY_HIT_TOLERANCE_PX` since a handle is a small target that must still
 * be easy to grab with a mouse. */
export const HANDLE_HIT_RADIUS_PX = 8;

/** Pixels a pointer may be from a line/edge and still count as hitting the
 * drawable's body. Matches the tolerance the click-to-select hit test used
 * before move mode existed. */
export const BODY_HIT_TOLERANCE_PX = 6;

/** Padding, in pixels, between a label's text and its background box's
 * edge. */
export const LABEL_PADDING_PX = 4;

/** Pixels between a label's anchor point and its box, for the `above`/
 * `below` anchors — `center` needs no gap since the box is centred on the
 * point itself. */
export const LABEL_GAP_PX = 6;

/** One drawable's geometry in screen pixels, ready to paint and hit-test.
 * A `level` has no `x` of its own (it spans the full pane width), so
 * `handleX` is only where its one handle is drawn/hit, never part of its
 * persisted geometry. A `segment`'s `anchorStart`/`anchorEnd` are its real,
 * draggable endpoints; `drawStart`/`drawEnd` are what actually gets
 * painted and hit — identical to the anchors when `extend` is `'none'`,
 * reaching past the pane's edge otherwise (`extendSegment`). A `label`'s
 * `box` is its painted background rectangle (`labelBoxRect`); `at` is its
 * one draggable handle. */
export type DrawableGeometry =
	| { kind: 'level'; y: number; handleX: number }
	| { kind: 'segment'; anchorStart: Point; anchorEnd: Point; drawStart: Point; drawEnd: Point }
	| { kind: 'box'; start: Point; end: Point }
	| { kind: 'label'; at: Point; box: { x: number; y: number; width: number; height: number } };

/** A `level`/`label` has one handle; a `segment`/`box` has the two anchors
 * it was drawn from. */
export type HandleId = 'point' | 'start' | 'end';

export type DrawingHit = { part: 'handle'; handle: HandleId } | { part: 'body' };

/** Every handle a drawable's pixel geometry has, and where it sits — the
 * one place that decides handle positions, so painting a selected
 * drawable's handles and hit-testing them can never disagree. */
export function handlePoints(geometry: DrawableGeometry): { handle: HandleId; point: Point }[] {
	switch (geometry.kind) {
		case 'level':
			return [{ handle: 'point', point: { x: geometry.handleX, y: geometry.y } }];
		case 'segment':
			return [
				{ handle: 'start', point: geometry.anchorStart },
				{ handle: 'end', point: geometry.anchorEnd }
			];
		case 'box':
			return [
				{ handle: 'start', point: geometry.start },
				{ handle: 'end', point: geometry.end }
			];
		case 'label':
			return [{ handle: 'point', point: geometry.at }];
	}
}

function distanceToSegment(p: Point, a: Point, b: Point): number {
	const dx = b.x - a.x;
	const dy = b.y - a.y;
	const lengthSq = dx * dx + dy * dy;
	if (lengthSq === 0) return Math.hypot(p.x - a.x, p.y - a.y);
	let t = ((p.x - a.x) * dx + (p.y - a.y) * dy) / lengthSq;
	t = Math.max(0, Math.min(1, t));
	return Math.hypot(p.x - (a.x + t * dx), p.y - (a.y + t * dy));
}

function pointInRect(point: Point, rect: { x: number; y: number; width: number; height: number }): boolean {
	return point.x >= rect.x && point.x <= rect.x + rect.width && point.y >= rect.y && point.y <= rect.y + rect.height;
}

function hitsBody(geometry: DrawableGeometry, point: Point): boolean {
	switch (geometry.kind) {
		case 'level':
			// The level spans the whole pane width, so only the perpendicular
			// distance (in y) counts.
			return Math.abs(point.y - geometry.y) <= BODY_HIT_TOLERANCE_PX;
		case 'segment':
			// A ray/extended segment is hit anywhere along what is actually
			// drawn, not just between its two stored anchors.
			return distanceToSegment(point, geometry.drawStart, geometry.drawEnd) <= BODY_HIT_TOLERANCE_PX;
		case 'box': {
			// A box is a *zone* — the whole enclosed area is clickable, not
			// just its border.
			const left = Math.min(geometry.start.x, geometry.end.x) - BODY_HIT_TOLERANCE_PX;
			const right = Math.max(geometry.start.x, geometry.end.x) + BODY_HIT_TOLERANCE_PX;
			const top = Math.min(geometry.start.y, geometry.end.y) - BODY_HIT_TOLERANCE_PX;
			const bottom = Math.max(geometry.start.y, geometry.end.y) + BODY_HIT_TOLERANCE_PX;
			return point.x >= left && point.x <= right && point.y >= top && point.y <= bottom;
		}
		case 'label':
			return pointInRect(point, geometry.box);
	}
}

/** Reports whether `point` lands on one of `geometry`'s handles, on its
 * body, or on neither — handles are checked first since a handle sits on
 * top of (and within the tolerance of) the body it belongs to. */
export function hitTestDrawable(geometry: DrawableGeometry, point: Point): DrawingHit | null {
	for (const { handle, point: hp } of handlePoints(geometry)) {
		if (Math.hypot(point.x - hp.x, point.y - hp.y) <= HANDLE_HIT_RADIUS_PX) {
			return { part: 'handle', handle };
		}
	}
	return hitsBody(geometry, point) ? { part: 'body' } : null;
}

/** A `segment`'s (or `level`'s notional) two drawn endpoints, extended past
 * the pane's edge in whichever direction `extend` asks for — used only to
 * build what gets *painted and hit*, never the draggable anchors
 * themselves (`handlePoints` always reports the true, unextended anchors).
 * Extends by a fixed multiple of the pane's own size rather than clipping
 * precisely to its edge: a canvas draws (and `distanceToSegment` measures)
 * coordinates far outside its visible bounds just as cheaply as ones
 * inside it, so there is no correctness reason to compute the exact
 * boundary crossing. */
export function extendSegment(start: Point, end: Point, extend: Extend, paneWidth: number, paneHeight: number): { start: Point; end: Point } {
	if (extend === 'none') return { start, end };
	const reach = (paneWidth + paneHeight) * 2;
	const extended = (from: Point, through: Point): Point => {
		const dx = through.x - from.x;
		const dy = through.y - from.y;
		const length = Math.hypot(dx, dy);
		if (length === 0) return through;
		const scale = reach / length;
		return { x: through.x + dx * scale, y: through.y + dy * scale };
	};
	return {
		start: extend === 'forward' ? start : extended(end, start),
		end: extend === 'backward' ? end : extended(start, end)
	};
}

/** A label's background box size, from its own text and font size alone —
 * a `~0.62em`-per-glyph estimate rather than a canvas `measureText`, so
 * this stays a pure function of the label's own fields. That is what lets
 * painting and hit-testing share one answer without either needing a
 * canvas context, and what keeps this module testable without a DOM at
 * all, like every other function here. */
export function labelBoxSize(text: string, fontSize: number): { width: number; height: number } {
	const width = Math.max(text.length * fontSize * 0.62, fontSize) + LABEL_PADDING_PX * 2;
	const height = fontSize + LABEL_PADDING_PX * 2;
	return { width, height };
}

/** A label's background box position, from its anchor point, its own size
 * (`labelBoxSize`) and where it sits relative to the anchor — the one place
 * that decides where a label's box goes, so drawing it and hit-testing it
 * can never disagree. */
export function labelBoxRect(
	at: Point,
	size: { width: number; height: number },
	anchor: LabelAnchor
): { x: number; y: number; width: number; height: number } {
	const { width, height } = size;
	const x = at.x - width / 2;
	switch (anchor) {
		case 'above':
			return { x, y: at.y - height - LABEL_GAP_PX, width, height };
		case 'below':
			return { x, y: at.y + LABEL_GAP_PX, width, height };
		case 'center':
			return { x, y: at.y - height / 2, width, height };
	}
}

/** Moves one handle to `to` (in the drawable's own time/price domain),
 * returning the drawable's new geometry. A `level`'s one handle only ever
 * carries a new price. */
export function moveHandle(drawable: ObjectDrawable, handle: HandleId, to: DrawablePoint): ObjectDrawable {
	switch (drawable.kind) {
		case 'level':
			return { ...drawable, price: to.value };
		case 'segment':
		case 'box':
			return handle === 'start' ? { ...drawable, a: to } : { ...drawable, b: to };
		case 'label':
			return { ...drawable, at: to };
	}
}

/** Shifts a whole drawable by `(dx, dy)` in its own time/price domain — a
 * body drag. A `level` has no time component to shift, so `dx` is ignored
 * for it. */
export function translateGeometry(drawable: ObjectDrawable, dx: number, dy: number): ObjectDrawable {
	switch (drawable.kind) {
		case 'level':
			return { ...drawable, price: drawable.price + dy };
		case 'segment':
		case 'box':
			return {
				...drawable,
				a: { time: drawable.a.time + dx, value: drawable.a.value + dy },
				b: { time: drawable.b.time + dx, value: drawable.b.value + dy }
			};
		case 'label':
			return { ...drawable, at: { time: drawable.at.time + dx, value: drawable.at.value + dy } };
	}
}
