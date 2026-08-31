// Pure hit-testing and drag maths for chart drawing objects — a horizontal
// line, a trend line, a rectangle (`DrawingKindTag` in `pane-runtime.ts`,
// matching `senken_workspace::DrawingKind` exactly). Kept apart from
// `drawing-primitive.ts` so the geometry itself is testable without a
// chart, a canvas, or `lightweight-charts` at all.
//
// `Point` is deliberately unitless: `drawing-primitive.ts` calls
// `hitTestDrawing` in screen pixels (where the hit radii below are
// meaningful) and calls `moveHandle`/`translateGeometry` again in the
// drawing's own (time, price) domain, so a drag never round-trips through a
// pixel position and back — only the geometry shape matters here, not which
// space its numbers live in.

/** A point in whatever 2D space the caller is working in — screen pixels
 * for hit-testing, (time, price) for the actual drag update. */
export interface Point {
	x: number;
	y: number;
}

/** Pixels (or domain-equivalent units) a pointer may be from a handle and
 * still grab it — larger than `BODY_HIT_TOLERANCE_PX` since a handle is a
 * small target that must still be easy to grab with a mouse. */
export const HANDLE_HIT_RADIUS_PX = 8;

/** Pixels a pointer may be from a line/edge and still count as hitting the
 * drawing's body. Matches the tolerance the click-to-select hit test used
 * before move mode existed. */
export const BODY_HIT_TOLERANCE_PX = 6;

/** One drawing's geometry, in whatever space the caller is working in (see
 * the module doc). A horizontal line has no `x` of its own — it spans the
 * full pane width — so `handleX` is only where its one handle is drawn/hit,
 * never part of its persisted geometry. */
export type DrawingGeometry =
	| { kind: 'horizontal_line'; y: number; handleX: number }
	| { kind: 'trend_line'; start: Point; end: Point }
	| { kind: 'rectangle'; start: Point; end: Point };

/** A horizontal line has one handle; a trend line and a rectangle each have
 * the two endpoints they were drawn from — matching the two dots
 * `drawing-primitive.ts` has always painted on a selected trend
 * line/rectangle, not a resize handle per visual corner. */
export type HandleId = 'point' | 'start' | 'end';

export type DrawingHit = { part: 'handle'; handle: HandleId } | { part: 'body' };

/** Every handle a drawing's geometry has, and where it sits — the one place
 * that decides handle positions, so painting a selected drawing's handles
 * and hit-testing them can never disagree. */
export function handlePoints(geometry: DrawingGeometry): { handle: HandleId; point: Point }[] {
	switch (geometry.kind) {
		case 'horizontal_line':
			return [{ handle: 'point', point: { x: geometry.handleX, y: geometry.y } }];
		case 'trend_line':
		case 'rectangle':
			return [
				{ handle: 'start', point: geometry.start },
				{ handle: 'end', point: geometry.end }
			];
		default: {
			const exhaustive: never = geometry;
			return exhaustive;
		}
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

function hitsBody(geometry: DrawingGeometry, point: Point): boolean {
	switch (geometry.kind) {
		case 'horizontal_line':
			// The line spans the whole pane width, so only the perpendicular
			// distance (in y) counts.
			return Math.abs(point.y - geometry.y) <= BODY_HIT_TOLERANCE_PX;
		case 'trend_line':
			return distanceToSegment(point, geometry.start, geometry.end) <= BODY_HIT_TOLERANCE_PX;
		case 'rectangle': {
			// A rectangle is a *zone* — the whole enclosed area is clickable,
			// not just its border.
			const left = Math.min(geometry.start.x, geometry.end.x) - BODY_HIT_TOLERANCE_PX;
			const right = Math.max(geometry.start.x, geometry.end.x) + BODY_HIT_TOLERANCE_PX;
			const top = Math.min(geometry.start.y, geometry.end.y) - BODY_HIT_TOLERANCE_PX;
			const bottom = Math.max(geometry.start.y, geometry.end.y) + BODY_HIT_TOLERANCE_PX;
			return point.x >= left && point.x <= right && point.y >= top && point.y <= bottom;
		}
		default: {
			const exhaustive: never = geometry;
			return exhaustive;
		}
	}
}

/** Reports whether `point` lands on one of `geometry`'s handles, on its
 * body, or on neither — handles are checked first since a handle sits on
 * top of (and within the tolerance of) the body it belongs to. */
export function hitTestDrawing(geometry: DrawingGeometry, point: Point): DrawingHit | null {
	for (const { handle, point: hp } of handlePoints(geometry)) {
		if (Math.hypot(point.x - hp.x, point.y - hp.y) <= HANDLE_HIT_RADIUS_PX) {
			return { part: 'handle', handle };
		}
	}
	return hitsBody(geometry, point) ? { part: 'body' } : null;
}

/** Moves one handle to `to`, returning the drawing's new geometry. A
 * horizontal line's one handle only ever carries a new `y` (its price) —
 * `handleX` is display placement, not geometry, and is left untouched. */
export function moveHandle(geometry: DrawingGeometry, handle: HandleId, to: Point): DrawingGeometry {
	switch (geometry.kind) {
		case 'horizontal_line':
			return { ...geometry, y: to.y };
		case 'trend_line':
		case 'rectangle':
			return handle === 'start' ? { ...geometry, start: to } : { ...geometry, end: to };
		default: {
			const exhaustive: never = geometry;
			return exhaustive;
		}
	}
}

/** Shifts a whole drawing by `(dx, dy)` — a body drag. A horizontal line
 * has no time component to shift, so `dx` is ignored for it. */
export function translateGeometry(geometry: DrawingGeometry, dx: number, dy: number): DrawingGeometry {
	switch (geometry.kind) {
		case 'horizontal_line':
			return { ...geometry, y: geometry.y + dy };
		case 'trend_line':
		case 'rectangle':
			return {
				...geometry,
				start: { x: geometry.start.x + dx, y: geometry.start.y + dy },
				end: { x: geometry.end.x + dx, y: geometry.end.y + dy }
			};
		default: {
			const exhaustive: never = geometry;
			return exhaustive;
		}
	}
}
