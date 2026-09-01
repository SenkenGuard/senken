import { describe, expect, test } from 'bun:test';
import { DrawingsPrimitive } from './drawing-primitive';
import type { DrawingRuntime } from './pane-runtime';
import type { IChartApi, ISeriesApi, SeriesAttachedParameter, Time } from 'lightweight-charts';

function horizontalLine(overrides: Partial<DrawingRuntime>): DrawingRuntime {
	return {
		id: 'd1',
		position: 0,
		kind: 'horizontal_line',
		visible: true,
		price: 100,
		color: '#7aa7e8',
		width: 2,
		lineStyle: 'SOLID',
		...overrides
	};
}

/** Attaches a primitive to a fake series whose `priceToCoordinate` is the
 * only chart API a horizontal line's geometry needs (`pixelGeometryFor`'s
 * `level` branch never calls into `timeToX`) — enough to make
 * `hitTestDetailed` a real, observable behaviour instead of always
 * returning `null` for want of a chart. */
function attach(primitive: DrawingsPrimitive, priceToCoordinate: (price: number) => number | null): void {
	primitive.attached({
		chart: {} as unknown as IChartApi,
		series: { priceToCoordinate } as unknown as ISeriesApi<'Candlestick'>,
		requestUpdate: () => {}
	} as unknown as SeriesAttachedParameter<Time>);
}

describe('setDrawings — a hidden drawing is dropped, not merely hidden', () => {
	test('a drawing with visible: false is not hit-testable at its own position, while a visible one at a different position still is', () => {
		const primitive = new DrawingsPrimitive();
		// price -> y is `500 - price`, so each price maps to a distinct, known pixel row.
		attach(primitive, (price) => 500 - price);
		primitive.setDrawings([
			horizontalLine({ id: 'shown', price: 100, visible: true }),
			horizontalLine({ id: 'hidden', price: 200, visible: false })
		]);

		expect(primitive.hitTestDetailed(0, 400)).toEqual({ id: 'shown', hit: { part: 'body' } });
		expect(primitive.hitTestDetailed(0, 300)).toBeNull();
	});
});

describe('setDrawings — keeps the incoming order, later entries painted (and hit) on top', () => {
	test('of two overlapping visible drawings, the one that comes later in the input array wins the hit test', () => {
		const primitive = new DrawingsPrimitive();
		attach(primitive, () => 250); // both drawings land on the same pixel row.
		primitive.setDrawings([
			horizontalLine({ id: 'first', price: 100, visible: true }),
			horizontalLine({ id: 'second', price: 200, visible: true })
		]);

		expect(primitive.hitTestDetailed(0, 250)).toEqual({ id: 'second', hit: { part: 'body' } });
	});
});
