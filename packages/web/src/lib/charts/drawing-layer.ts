import type { DrawingsPrimitive } from './drawing-primitive';
import type { DrawingHit } from './drawing-hit';

/** Finds the drawing under a pointer. Geometry stays outside the chart shell. */
export function drawingAt(
	primitive: DrawingsPrimitive | undefined,
	x: number,
	y: number
): { id: string; hit: DrawingHit } | null {
	return primitive?.hitTestDetailed(x, y) ?? null;
}
