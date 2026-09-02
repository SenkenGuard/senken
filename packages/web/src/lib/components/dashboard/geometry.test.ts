import { describe, expect, test } from 'bun:test';
import {
	cellWidthPx,
	clampRectToGrid,
	findOverlap,
	fitsWithinColumns,
	pixelDeltaToGridDelta,
	rectanglesOverlap,
	type GridMetrics,
	type GridRect
} from './geometry';

describe('rectanglesOverlap', () => {
	test('two rectangles sharing no cell at all do not overlap', () => {
		const a: GridRect = { x: 0, y: 0, width: 6, height: 4 };
		const b: GridRect = { x: 6, y: 0, width: 6, height: 4 };
		expect(rectanglesOverlap(a, b)).toBe(false);
	});

	test('shifting the adjacent rectangle one column left makes them overlap', () => {
		// Same two rectangles as above, but the second now shares column 5
		// with the first — this must flip the result, proving the check
		// actually looks at the shared column rather than always agreeing.
		const a: GridRect = { x: 0, y: 0, width: 6, height: 4 };
		const b: GridRect = { x: 5, y: 0, width: 6, height: 4 };
		expect(rectanglesOverlap(a, b)).toBe(true);
	});

	test('rectangles stacked in different rows do not overlap even at the same columns', () => {
		const a: GridRect = { x: 0, y: 0, width: 6, height: 4 };
		const b: GridRect = { x: 0, y: 4, width: 6, height: 4 };
		expect(rectanglesOverlap(a, b)).toBe(false);
	});

	test('one rectangle fully containing another counts as overlapping', () => {
		const outer: GridRect = { x: 0, y: 0, width: 12, height: 8 };
		const inner: GridRect = { x: 4, y: 2, width: 2, height: 2 };
		expect(rectanglesOverlap(outer, inner)).toBe(true);
	});
});

describe('fitsWithinColumns', () => {
	test('a rectangle ending exactly at the grid edge fits', () => {
		expect(fitsWithinColumns({ x: 6, y: 0, width: 6, height: 4 }, 12)).toBe(true);
	});

	test('a rectangle one column past the edge does not fit', () => {
		expect(fitsWithinColumns({ x: 7, y: 0, width: 6, height: 4 }, 12)).toBe(false);
	});

	test('zero width or height never fits, regardless of position', () => {
		expect(fitsWithinColumns({ x: 0, y: 0, width: 0, height: 4 }, 12)).toBe(false);
		expect(fitsWithinColumns({ x: 0, y: 0, width: 6, height: 0 }, 12)).toBe(false);
	});
});

describe('findOverlap', () => {
	test('finds the first overlapping pair by index', () => {
		// rects[0] and rects[1] are adjacent, not overlapping; rects[2]
		// overlaps rects[0] (columns 3 is shared) before the scan ever
		// reaches the rects[1]/rects[2] pair, so (0, 2) is the first pair
		// found, not (1, 2).
		const rects: GridRect[] = [
			{ x: 0, y: 0, width: 4, height: 2 },
			{ x: 4, y: 0, width: 4, height: 2 },
			{ x: 3, y: 0, width: 4, height: 2 }
		];
		expect(findOverlap(rects)).toEqual([0, 2]);
	});

	test('returns null when nothing overlaps', () => {
		const rects: GridRect[] = [
			{ x: 0, y: 0, width: 4, height: 2 },
			{ x: 4, y: 0, width: 4, height: 2 }
		];
		expect(findOverlap(rects)).toBeNull();
	});
});

describe('cellWidthPx and pixelDeltaToGridDelta', () => {
	function metrics(containerWidthPx: number, columns = 12): GridMetrics {
		return { containerWidthPx, columns, rowHeightPx: 48, gapPx: 12 };
	}

	test('the same stored rectangle occupies a different pixel width at a different container width', () => {
		// This is the "layout saved at one window size reads correctly at
		// another" property: nothing about the stored (x, y, width, height)
		// changes, only the pixel size a caller derives from it changes with
		// the container. (1200 - 12 * 11) / 12 = 89; (2400 - 12 * 11) / 12 = 189.
		expect(cellWidthPx(metrics(1200))).toBeCloseTo(89, 5);
		expect(cellWidthPx(metrics(2400))).toBeCloseTo(189, 5);
	});

	test('a drag distance of exactly one cell plus its gap converts to a delta of one', () => {
		const m = metrics(1200);
		const oneCell = cellWidthPx(m) + m.gapPx;
		expect(pixelDeltaToGridDelta(oneCell, 0, m)).toEqual({ dx: 1, dy: 0 });
	});

	test('a drag distance less than half a cell rounds back to zero', () => {
		const m = metrics(1200);
		const halfCell = (cellWidthPx(m) + m.gapPx) / 2;
		expect(pixelDeltaToGridDelta(halfCell - 1, 0, m).dx).toBe(0);
	});

	test('a drag distance more than half a cell rounds up to one', () => {
		const m = metrics(1200);
		const halfCell = (cellWidthPx(m) + m.gapPx) / 2;
		expect(pixelDeltaToGridDelta(halfCell + 1, 0, m).dx).toBe(1);
	});
});

describe('clampRectToGrid', () => {
	test('leaves an already-valid rectangle untouched', () => {
		const rect: GridRect = { x: 2, y: 1, width: 4, height: 3 };
		expect(clampRectToGrid(rect, 12)).toEqual(rect);
	});

	test('pulls x back so the widget never runs past the right edge', () => {
		const rect: GridRect = { x: 10, y: 0, width: 6, height: 3 };
		expect(clampRectToGrid(rect, 12)).toEqual({ x: 6, y: 0, width: 6, height: 3 });
	});

	test('shrinks a widget wider than the grid itself down to the full grid width', () => {
		const rect: GridRect = { x: 0, y: 0, width: 20, height: 3 };
		expect(clampRectToGrid(rect, 12)).toEqual({ x: 0, y: 0, width: 12, height: 3 });
	});

	test('never produces a negative x or y even from negative input', () => {
		const rect: GridRect = { x: -5, y: -2, width: 4, height: 3 };
		expect(clampRectToGrid(rect, 12)).toEqual({ x: 0, y: 0, width: 4, height: 3 });
	});
});
