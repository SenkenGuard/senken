// Pure grid math for the dashboard's widget grid — no DOM, no fetch, no
// lifecycle. Placement itself is rendered with native CSS Grid
// (`grid-column`/`grid-row`, computed from the same integers this module
// works with), so the only pixel math that exists at all is turning a
// pointer's drag distance into a grid delta while a drag is in progress.
// Nothing here ever bakes a pixel value into anything stored: a cell size
// is always measured fresh (`GridMetrics.cellWidth`) from the container's
// *current* size when a drag starts, never carried over from a previous
// drag or a previous window size — the same "derive from what you hold"
// discipline every other stale-viewport bug in this project traces back
// to violating.

/** The fixed row height every dashboard grid renders at, in pixels. A
 * widget's `height` is a row *count*; this is what turns that count into
 * something CSS Grid's `grid-auto-rows` can use. */
export const DEFAULT_ROW_HEIGHT_PX = 44;

/** The gap between grid cells, in pixels, both axes. */
export const DEFAULT_GAP_PX = 12;

/** A widget's placement, in grid cells — matches the server's own
 * `position_x`/`position_y`/`width`/`height` columns exactly. Never
 * pixels. */
export interface GridRect {
	x: number;
	y: number;
	width: number;
	height: number;
}

/** Two axis-aligned grid rectangles overlap iff they overlap on both
 * axes — the exact same check `senken_dashboard`'s own
 * `replace_layout` runs in Rust before writing anything, mirrored here so
 * a drag can be rejected before it is ever sent to the server. */
export function rectanglesOverlap(a: GridRect, b: GridRect): boolean {
	return a.x < b.x + b.width && b.x < a.x + a.width && a.y < b.y + b.height && b.y < a.y + a.height;
}

/** `true` if `rect` fits within `0..columns` horizontally and has
 * positive area — the same bound `senken_dashboard::DashboardError::OutOfBounds`/
 * `ZeroSizedWidget` reject server-side. */
export function fitsWithinColumns(rect: GridRect, columns: number): boolean {
	return rect.x >= 0 && rect.y >= 0 && rect.width > 0 && rect.height > 0 && rect.x + rect.width <= columns;
}

/** The first overlapping pair found in `rects` (by index into the array),
 * or `null` if none overlap. */
export function findOverlap(rects: readonly GridRect[]): [number, number] | null {
	for (let i = 0; i < rects.length; i++) {
		for (let j = i + 1; j < rects.length; j++) {
			if (rectanglesOverlap(rects[i], rects[j])) return [i, j];
		}
	}
	return null;
}

/** A grid's current pixel geometry, always measured fresh from the live
 * container — see this module's own header comment for why nothing here
 * is ever cached across a resize. */
export interface GridMetrics {
	/** The grid container's current content width, in pixels
	 * (`element.getBoundingClientRect().width`). */
	containerWidthPx: number;
	/** How many columns the workspace's grid uses. */
	columns: number;
	/** One grid row's fixed height, in pixels. */
	rowHeightPx: number;
	/** The gap between cells, in pixels, both axes. */
	gapPx: number;
}

/** One grid column's width in pixels, derived from the container's
 * *current* width — never a value carried over from an earlier
 * measurement. */
export function cellWidthPx(metrics: GridMetrics): number {
	const usable = metrics.containerWidthPx - metrics.gapPx * (metrics.columns - 1);
	return usable / metrics.columns;
}

/** Converts a pointer's drag distance (in pixels, since the drag began)
 * into a whole-cell grid delta, rounding to the nearest cell rather than
 * truncating so a small drag past the halfway point of a cell already
 * commits to the next one. */
export function pixelDeltaToGridDelta(
	deltaXPx: number,
	deltaYPx: number,
	metrics: GridMetrics
): { dx: number; dy: number } {
	const cellW = cellWidthPx(metrics) + metrics.gapPx;
	const cellH = metrics.rowHeightPx + metrics.gapPx;
	return {
		dx: Math.round(deltaXPx / cellW),
		dy: Math.round(deltaYPx / cellH)
	};
}

/** Clamps `rect` to a valid placement on a `columns`-wide grid: width no
 * wider than the grid itself, `x` pulled back so the widget never runs
 * past the right edge, `y` and `height` never negative or zero. Used
 * while a drag is in progress so the widget being dragged never visually
 * leaves the grid, even before the final position is validated against
 * every other widget. */
export function clampRectToGrid(rect: GridRect, columns: number): GridRect {
	const width = Math.max(1, Math.min(rect.width, columns));
	const height = Math.max(1, rect.height);
	const x = Math.max(0, Math.min(rect.x, columns - width));
	const y = Math.max(0, rect.y);
	return { x, y, width, height };
}
