import { describe, expect, test } from 'bun:test';
import {
	BODY_HIT_TOLERANCE_PX,
	HANDLE_HIT_RADIUS_PX,
	handlePoints,
	hitTestDrawing,
	moveHandle,
	translateGeometry,
	type DrawingGeometry
} from './drawing-hit';

describe('handlePoints — one source of truth for where a handle sits', () => {
	test('a horizontal line has one handle, at its own handleX', () => {
		const geometry: DrawingGeometry = { kind: 'horizontal_line', y: 50, handleX: 120 };
		expect(handlePoints(geometry)).toEqual([{ handle: 'point', point: { x: 120, y: 50 } }]);
	});

	test('a trend line has its two endpoints', () => {
		const geometry: DrawingGeometry = { kind: 'trend_line', start: { x: 0, y: 0 }, end: { x: 100, y: 40 } };
		expect(handlePoints(geometry)).toEqual([
			{ handle: 'start', point: { x: 0, y: 0 } },
			{ handle: 'end', point: { x: 100, y: 40 } }
		]);
	});

	test('a rectangle has its two stored corners, not four resize corners', () => {
		const geometry: DrawingGeometry = { kind: 'rectangle', start: { x: 10, y: 10 }, end: { x: 90, y: 60 } };
		expect(handlePoints(geometry)).toEqual([
			{ handle: 'start', point: { x: 10, y: 10 } },
			{ handle: 'end', point: { x: 90, y: 60 } }
		]);
	});
});

describe('hitTestDrawing — horizontal line', () => {
	const geometry: DrawingGeometry = { kind: 'horizontal_line', y: 200, handleX: 150 };

	test('a click on the handle reports the handle, not the body', () => {
		expect(hitTestDrawing(geometry, { x: 150, y: 200 })).toEqual({ part: 'handle', handle: 'point' });
	});

	test('a click anywhere else on the line (any x) reports the body', () => {
		expect(hitTestDrawing(geometry, { x: 5, y: 200 })).toEqual({ part: 'body' });
		expect(hitTestDrawing(geometry, { x: 900, y: 199 })).toEqual({ part: 'body' });
	});

	test('a click off the line reports nothing', () => {
		expect(hitTestDrawing(geometry, { x: 5, y: 200 + BODY_HIT_TOLERANCE_PX + 1 })).toBeNull();
	});

	test('a click just inside the handle radius still reports the handle', () => {
		const nearHandle = { x: geometry.handleX + HANDLE_HIT_RADIUS_PX - 1, y: geometry.y };
		expect(hitTestDrawing(geometry, nearHandle)).toEqual({ part: 'handle', handle: 'point' });
	});
});

describe('hitTestDrawing — trend line', () => {
	const geometry: DrawingGeometry = { kind: 'trend_line', start: { x: 0, y: 0 }, end: { x: 100, y: 100 } };

	test('a click on an endpoint reports that handle', () => {
		expect(hitTestDrawing(geometry, { x: 0, y: 0 })).toEqual({ part: 'handle', handle: 'start' });
		expect(hitTestDrawing(geometry, { x: 100, y: 100 })).toEqual({ part: 'handle', handle: 'end' });
	});

	test('a click on the segment, away from either endpoint, reports the body', () => {
		expect(hitTestDrawing(geometry, { x: 50, y: 50 })).toEqual({ part: 'body' });
	});

	test('a click far from the segment reports nothing', () => {
		expect(hitTestDrawing(geometry, { x: 50, y: 50 + BODY_HIT_TOLERANCE_PX + 10 })).toBeNull();
	});
});

describe('hitTestDrawing — rectangle', () => {
	const geometry: DrawingGeometry = { kind: 'rectangle', start: { x: 20, y: 20 }, end: { x: 80, y: 60 } };

	test('a click on a stored corner reports that handle', () => {
		expect(hitTestDrawing(geometry, { x: 20, y: 20 })).toEqual({ part: 'handle', handle: 'start' });
		expect(hitTestDrawing(geometry, { x: 80, y: 60 })).toEqual({ part: 'handle', handle: 'end' });
	});

	test('a click anywhere inside the zone reports the body — it is a filled zone, not just a border', () => {
		expect(hitTestDrawing(geometry, { x: 50, y: 40 })).toEqual({ part: 'body' });
	});

	test('a click just outside the zone (past tolerance) reports nothing', () => {
		expect(hitTestDrawing(geometry, { x: 80 + BODY_HIT_TOLERANCE_PX + 1, y: 40 })).toBeNull();
	});

	test('a rectangle drawn bottom-right to top-left still hit-tests as the same zone', () => {
		const reversed: DrawingGeometry = { kind: 'rectangle', start: { x: 80, y: 60 }, end: { x: 20, y: 20 } };
		expect(hitTestDrawing(reversed, { x: 50, y: 40 })).toEqual({ part: 'body' });
	});
});

describe('moveHandle', () => {
	test('a horizontal line only ever moves in y — handleX is untouched', () => {
		const geometry: DrawingGeometry = { kind: 'horizontal_line', y: 10, handleX: 300 };
		expect(moveHandle(geometry, 'point', { x: 999, y: 55 })).toEqual({ kind: 'horizontal_line', y: 55, handleX: 300 });
	});

	test('a trend line\'s start handle moves only its start point', () => {
		const geometry: DrawingGeometry = { kind: 'trend_line', start: { x: 0, y: 0 }, end: { x: 100, y: 100 } };
		expect(moveHandle(geometry, 'start', { x: 5, y: 9 })).toEqual({
			kind: 'trend_line',
			start: { x: 5, y: 9 },
			end: { x: 100, y: 100 }
		});
	});

	test('a rectangle\'s end handle moves only its end point', () => {
		const geometry: DrawingGeometry = { kind: 'rectangle', start: { x: 0, y: 0 }, end: { x: 100, y: 100 } };
		expect(moveHandle(geometry, 'end', { x: 40, y: 70 })).toEqual({
			kind: 'rectangle',
			start: { x: 0, y: 0 },
			end: { x: 40, y: 70 }
		});
	});
});

describe('translateGeometry', () => {
	test('a horizontal line ignores dx — it has no time component to shift', () => {
		const geometry: DrawingGeometry = { kind: 'horizontal_line', y: 10, handleX: 300 };
		expect(translateGeometry(geometry, 500, -3)).toEqual({ kind: 'horizontal_line', y: 7, handleX: 300 });
	});

	test('a trend line shifts both endpoints by the same delta', () => {
		const geometry: DrawingGeometry = { kind: 'trend_line', start: { x: 0, y: 0 }, end: { x: 100, y: 100 } };
		expect(translateGeometry(geometry, 10, -5)).toEqual({
			kind: 'trend_line',
			start: { x: 10, y: -5 },
			end: { x: 110, y: 95 }
		});
	});

	test('a rectangle shifts both stored corners by the same delta', () => {
		const geometry: DrawingGeometry = { kind: 'rectangle', start: { x: 20, y: 20 }, end: { x: 80, y: 60 } };
		expect(translateGeometry(geometry, -4, 8)).toEqual({
			kind: 'rectangle',
			start: { x: 16, y: 28 },
			end: { x: 76, y: 68 }
		});
	});
});
