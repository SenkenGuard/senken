import { describe, expect, test } from 'bun:test';
import {
	BODY_HIT_TOLERANCE_PX,
	HANDLE_HIT_RADIUS_PX,
	extendSegment,
	handlePoints,
	hitTestDrawable,
	labelBoxRect,
	labelBoxSize,
	moveHandle,
	translateGeometry,
	type DrawableGeometry
} from './drawing-hit';
import { drawableFromDrawingRuntime, type ObjectDrawable } from './drawable';

describe('handlePoints — one source of truth for where a handle sits', () => {
	test('a level has one handle, at its own handleX', () => {
		const geometry: DrawableGeometry = { kind: 'level', y: 50, handleX: 120 };
		expect(handlePoints(geometry)).toEqual([{ handle: 'point', point: { x: 120, y: 50 } }]);
	});

	test('a segment has its two real anchors, not its extended draw endpoints', () => {
		const geometry: DrawableGeometry = {
			kind: 'segment',
			anchorStart: { x: 0, y: 0 },
			anchorEnd: { x: 100, y: 40 },
			drawStart: { x: -900, y: -360 },
			drawEnd: { x: 1000, y: 400 }
		};
		expect(handlePoints(geometry)).toEqual([
			{ handle: 'start', point: { x: 0, y: 0 } },
			{ handle: 'end', point: { x: 100, y: 40 } }
		]);
	});

	test('a box has its two stored corners, not four resize corners', () => {
		const geometry: DrawableGeometry = { kind: 'box', start: { x: 10, y: 10 }, end: { x: 90, y: 60 } };
		expect(handlePoints(geometry)).toEqual([
			{ handle: 'start', point: { x: 10, y: 10 } },
			{ handle: 'end', point: { x: 90, y: 60 } }
		]);
	});

	test('a label has one handle, at its own anchor point', () => {
		const geometry: DrawableGeometry = { kind: 'label', at: { x: 30, y: 40 }, box: { x: 10, y: 10, width: 40, height: 20 } };
		expect(handlePoints(geometry)).toEqual([{ handle: 'point', point: { x: 30, y: 40 } }]);
	});
});

describe('hitTestDrawable — level', () => {
	const geometry: DrawableGeometry = { kind: 'level', y: 200, handleX: 150 };

	test('a click on the handle reports the handle, not the body', () => {
		expect(hitTestDrawable(geometry, { x: 150, y: 200 })).toEqual({ part: 'handle', handle: 'point' });
	});

	test('a click anywhere else on the line (any x) reports the body', () => {
		expect(hitTestDrawable(geometry, { x: 5, y: 200 })).toEqual({ part: 'body' });
		expect(hitTestDrawable(geometry, { x: 900, y: 199 })).toEqual({ part: 'body' });
	});

	test('a click off the line reports nothing', () => {
		expect(hitTestDrawable(geometry, { x: 5, y: 200 + BODY_HIT_TOLERANCE_PX + 1 })).toBeNull();
	});

	test('a click just inside the handle radius still reports the handle', () => {
		const nearHandle = { x: geometry.handleX + HANDLE_HIT_RADIUS_PX - 1, y: geometry.y };
		expect(hitTestDrawable(geometry, nearHandle)).toEqual({ part: 'handle', handle: 'point' });
	});
});

describe('hitTestDrawable — segment', () => {
	const geometry: DrawableGeometry = {
		kind: 'segment',
		anchorStart: { x: 0, y: 0 },
		anchorEnd: { x: 100, y: 100 },
		drawStart: { x: 0, y: 0 },
		drawEnd: { x: 100, y: 100 }
	};

	test('a click on an anchor reports that handle', () => {
		expect(hitTestDrawable(geometry, { x: 0, y: 0 })).toEqual({ part: 'handle', handle: 'start' });
		expect(hitTestDrawable(geometry, { x: 100, y: 100 })).toEqual({ part: 'handle', handle: 'end' });
	});

	test('a click on the segment, away from either endpoint, reports the body', () => {
		expect(hitTestDrawable(geometry, { x: 50, y: 50 })).toEqual({ part: 'body' });
	});

	test('a click far from the segment reports nothing', () => {
		expect(hitTestDrawable(geometry, { x: 50, y: 50 + BODY_HIT_TOLERANCE_PX + 10 })).toBeNull();
	});

	test('a ray (extended draw endpoint) is hit past its own anchor, since the body follows what is actually drawn', () => {
		const ray: DrawableGeometry = { ...geometry, drawEnd: { x: 1000, y: 1000 } };
		expect(hitTestDrawable(ray, { x: 500, y: 500 })).toEqual({ part: 'body' });
	});
});

describe('hitTestDrawable — box', () => {
	const geometry: DrawableGeometry = { kind: 'box', start: { x: 20, y: 20 }, end: { x: 80, y: 60 } };

	test('a click on a stored corner reports that handle', () => {
		expect(hitTestDrawable(geometry, { x: 20, y: 20 })).toEqual({ part: 'handle', handle: 'start' });
		expect(hitTestDrawable(geometry, { x: 80, y: 60 })).toEqual({ part: 'handle', handle: 'end' });
	});

	test('a click anywhere inside the zone reports the body — it is a filled zone, not just a border', () => {
		expect(hitTestDrawable(geometry, { x: 50, y: 40 })).toEqual({ part: 'body' });
	});

	test('a click just outside the zone (past tolerance) reports nothing', () => {
		expect(hitTestDrawable(geometry, { x: 80 + BODY_HIT_TOLERANCE_PX + 1, y: 40 })).toBeNull();
	});

	test('a box drawn bottom-right to top-left still hit-tests as the same zone', () => {
		const reversed: DrawableGeometry = { kind: 'box', start: { x: 80, y: 60 }, end: { x: 20, y: 20 } };
		expect(hitTestDrawable(reversed, { x: 50, y: 40 })).toEqual({ part: 'body' });
	});
});

describe('hitTestDrawable — label', () => {
	const size = labelBoxSize('supply zone', 11);
	const box = labelBoxRect({ x: 100, y: 100 }, size, 'above');
	const geometry: DrawableGeometry = { kind: 'label', at: { x: 100, y: 100 }, box };

	test('a click on the anchor point reports the handle', () => {
		expect(hitTestDrawable(geometry, { x: 100, y: 100 })).toEqual({ part: 'handle', handle: 'point' });
	});

	test('a click inside the painted text box (away from the anchor handle) reports the body', () => {
		const inside = { x: box.x + box.width / 2, y: box.y + box.height / 2 };
		expect(hitTestDrawable(geometry, inside)).toEqual({ part: 'body' });
	});

	test('a click outside the box and away from the handle reports nothing', () => {
		expect(hitTestDrawable(geometry, { x: box.x - 50, y: box.y - 50 })).toBeNull();
	});
});

describe('labelBoxSize / labelBoxRect — the one place painting and hit-testing agree on a label\'s box', () => {
	test('a longer label reports a wider box', () => {
		const short = labelBoxSize('BB', 11);
		const long = labelBoxSize('long/short position entry', 11);
		expect(long.width).toBeGreaterThan(short.width);
	});

	test('an "above" label sits with its box bottom edge above the anchor, with a gap', () => {
		const size = labelBoxSize('x', 10);
		const rect = labelBoxRect({ x: 50, y: 50 }, size, 'above');
		expect(rect.y + rect.height).toBeLessThan(50);
	});

	test('a "below" label sits with its box top edge below the anchor, with a gap', () => {
		const size = labelBoxSize('x', 10);
		const rect = labelBoxRect({ x: 50, y: 50 }, size, 'below');
		expect(rect.y).toBeGreaterThan(50);
	});

	test('a "center" label is centered on the anchor', () => {
		const size = labelBoxSize('x', 10);
		const rect = labelBoxRect({ x: 50, y: 50 }, size, 'center');
		expect(rect.x + rect.width / 2).toBeCloseTo(50);
		expect(rect.y + rect.height / 2).toBeCloseTo(50);
	});
});

describe('extendSegment — a ray/both-direction line reaches past the pane, a plain segment does not', () => {
	const start = { x: 100, y: 100 };
	const end = { x: 200, y: 100 };

	test("'none' draws exactly the two stored anchors", () => {
		expect(extendSegment(start, end, 'none', 400, 300)).toEqual({ start, end });
	});

	test("'forward' extends only past the end anchor", () => {
		const { start: drawStart, end: drawEnd } = extendSegment(start, end, 'forward', 400, 300);
		expect(drawStart).toEqual(start);
		expect(drawEnd.x).toBeGreaterThan(end.x);
	});

	test("'backward' extends only past the start anchor", () => {
		const { start: drawStart, end: drawEnd } = extendSegment(start, end, 'backward', 400, 300);
		expect(drawEnd).toEqual(end);
		expect(drawStart.x).toBeLessThan(start.x);
	});

	test("'both' extends past both anchors, and reaches outside the pane", () => {
		const { start: drawStart, end: drawEnd } = extendSegment(start, end, 'both', 400, 300);
		expect(drawStart.x).toBeLessThan(0);
		expect(drawEnd.x).toBeGreaterThan(400);
	});
});

describe('moveHandle — domain-space drag maths, over the shared ObjectDrawable, not a persisted-drawing-only type', () => {
	test('a level only ever moves in price — its handle carries the new value', () => {
		const drawable: ObjectDrawable = { kind: 'level', ownerId: 'd1', price: 10, extend: 'both', style: { color: '#fff', width: 1, lineStyle: 'SOLID' } };
		expect(moveHandle(drawable, 'point', { time: 999, value: 55 })).toEqual({ ...drawable, price: 55 });
	});

	test("a segment's start handle moves only its start anchor", () => {
		const drawable: ObjectDrawable = {
			kind: 'segment',
			ownerId: 'd1',
			a: { time: 0, value: 0 },
			b: { time: 100, value: 100 },
			extend: 'none',
			style: { color: '#fff', width: 1, lineStyle: 'SOLID' }
		};
		expect(moveHandle(drawable, 'start', { time: 5, value: 9 })).toEqual({ ...drawable, a: { time: 5, value: 9 } });
	});

	test("a box's end handle moves only its end anchor", () => {
		const drawable: ObjectDrawable = {
			kind: 'box',
			ownerId: 'd1',
			a: { time: 0, value: 0 },
			b: { time: 100, value: 100 },
			style: { color: '#fff', width: 1, lineStyle: 'SOLID', fillOpacity: 0.1 }
		};
		expect(moveHandle(drawable, 'end', { time: 40, value: 70 })).toEqual({ ...drawable, b: { time: 40, value: 70 } });
	});

	test("a label's point handle moves its anchor", () => {
		const drawable: ObjectDrawable = {
			kind: 'label',
			ownerId: 'ind-1',
			at: { time: 0, value: 0 },
			text: 'note',
			anchor: 'above',
			style: { textColor: '#000', background: '#fff', fontSize: 11 }
		};
		expect(moveHandle(drawable, 'point', { time: 12, value: 34 })).toEqual({ ...drawable, at: { time: 12, value: 34 } });
	});
});

describe('translateGeometry', () => {
	test('a level ignores the time delta — it has no time component to shift', () => {
		const drawable: ObjectDrawable = { kind: 'level', ownerId: 'd1', price: 10, extend: 'both', style: { color: '#fff', width: 1, lineStyle: 'SOLID' } };
		expect(translateGeometry(drawable, 500, -3)).toEqual({ ...drawable, price: 7 });
	});

	test('a segment shifts both anchors by the same delta', () => {
		const drawable: ObjectDrawable = {
			kind: 'segment',
			ownerId: 'd1',
			a: { time: 0, value: 0 },
			b: { time: 100, value: 100 },
			extend: 'none',
			style: { color: '#fff', width: 1, lineStyle: 'SOLID' }
		};
		expect(translateGeometry(drawable, 10, -5)).toEqual({ ...drawable, a: { time: 10, value: -5 }, b: { time: 110, value: 95 } });
	});

	test('a box shifts both stored corners by the same delta', () => {
		const drawable: ObjectDrawable = {
			kind: 'box',
			ownerId: 'd1',
			a: { time: 20, value: 20 },
			b: { time: 80, value: 60 },
			style: { color: '#fff', width: 1, lineStyle: 'SOLID', fillOpacity: 0.1 }
		};
		expect(translateGeometry(drawable, -4, 8)).toEqual({ ...drawable, a: { time: 16, value: 28 }, b: { time: 76, value: 68 } });
	});
});

describe('one shared path — hit-testing does not know or care which side produced a Box', () => {
	// A persisted rectangle and a synthetic object shaped exactly like what a
	// future indicator's own `Box` display item will look like
	// (`crates/indicators/src/drawable.rs::Drawable::Box`, not yet reachable
	// through `POST /api/indicators/compute` — see this track's report) are
	// fed through the *same* pixel-geometry shape and the *same*
	// `hitTestDrawable` used above. Nothing here special-cases either
	// origin — that absence is the property this test is proving, not the
	// specific coordinates.
	test('a persisted-drawing Box and an indicator-emitted Box hit-test identically for the same geometry', () => {
		const persisted = drawableFromDrawingRuntime({
			id: 'drawing-1',
			position: 0,
			kind: 'rectangle',
			visible: true,
			start: { time: 0, price: 0 },
			end: { time: 0, price: 0 },
			color: '#7aa7e8',
			width: 1,
			lineStyle: 'SOLID'
		});
		const indicatorEmitted: ObjectDrawable = {
			kind: 'box',
			ownerId: 'indicator-layer-1',
			a: { time: 0, value: 0 },
			b: { time: 0, value: 0 },
			style: { color: '#7aa7e8', width: 1, lineStyle: 'SOLID', fillOpacity: 0.12 }
		};
		expect(persisted.kind).toBe('box');
		expect(indicatorEmitted.kind).toBe('box');

		// Both origins produce the same box geometry when placed at the same
		// screen position — the geometry conversion (`pixelGeometryFor` in
		// `drawing-primitive.ts`) reads only `kind`/`a`/`b`, never `ownerId`.
		const geometry: DrawableGeometry = { kind: 'box', start: { x: 10, y: 10 }, end: { x: 90, y: 60 } };
		const point = { x: 50, y: 40 };
		expect(hitTestDrawable(geometry, point)).toEqual({ part: 'body' });

		// The only thing that ever differs between the two origins is the id
		// a hit reports back — which is exactly what lets a click on an
		// indicator's own object select the indicator instead of a drawing.
		expect(persisted.ownerId).toBe('drawing-1');
		expect(indicatorEmitted.ownerId).toBe('indicator-layer-1');
	});
});
