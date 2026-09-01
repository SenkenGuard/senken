import { describe, expect, test } from 'bun:test';
import { DEFAULT_BOX_FILL_OPACITY, DEFAULT_LABEL_STYLE, drawableFromDrawingRuntime, drawablesFromDrawingRuntime, patchFromDrawable, type ObjectDrawable } from './drawable';
import type { DrawingRuntime } from './pane-runtime';

function baseDrawing(overrides: Partial<DrawingRuntime>): DrawingRuntime {
	return {
		id: 'd1',
		position: 0,
		kind: 'horizontal_line',
		visible: true,
		color: '#7aa7e8',
		width: 2,
		lineStyle: 'SOLID',
		...overrides
	};
}

describe('drawableFromDrawingRuntime — one adapter, no per-kind renderer', () => {
	test('a horizontal line becomes a level that spans the pane both ways', () => {
		const drawing = baseDrawing({ kind: 'horizontal_line', price: 42_000 });
		expect(drawableFromDrawingRuntime(drawing)).toEqual({
			kind: 'level',
			ownerId: 'd1',
			price: 42_000,
			extend: 'both',
			style: { color: '#7aa7e8', width: 2, lineStyle: 'SOLID' }
		});
	});

	test('a trend line becomes a segment that does not extend past its anchors', () => {
		const drawing = baseDrawing({
			kind: 'trend_line',
			start: { time: 100, price: 10 },
			end: { time: 200, price: 20 }
		});
		expect(drawableFromDrawingRuntime(drawing)).toEqual({
			kind: 'segment',
			ownerId: 'd1',
			a: { time: 100, value: 10 },
			b: { time: 200, value: 20 },
			extend: 'none',
			style: { color: '#7aa7e8', width: 2, lineStyle: 'SOLID' }
		});
	});

	test('a rectangle becomes a box with the same fill every rectangle has always painted', () => {
		const drawing = baseDrawing({
			kind: 'rectangle',
			start: { time: 100, price: 10 },
			end: { time: 200, price: 30 }
		});
		expect(drawableFromDrawingRuntime(drawing)).toEqual({
			kind: 'box',
			ownerId: 'd1',
			a: { time: 100, value: 10 },
			b: { time: 200, value: 30 },
			style: { color: '#7aa7e8', width: 2, lineStyle: 'SOLID', fillOpacity: DEFAULT_BOX_FILL_OPACITY }
		});
	});

	test('a ray becomes a segment that extends forward past its second anchor', () => {
		const drawing = baseDrawing({
			kind: 'ray',
			start: { time: 100, price: 10 },
			end: { time: 200, price: 20 }
		});
		expect(drawableFromDrawingRuntime(drawing)).toEqual({
			kind: 'segment',
			ownerId: 'd1',
			a: { time: 100, value: 10 },
			b: { time: 200, value: 20 },
			extend: 'forward',
			style: { color: '#7aa7e8', width: 2, lineStyle: 'SOLID' }
		});
	});

	test('a text note becomes a label at its one anchor, textColor from the drawing\'s own colour', () => {
		const drawing = baseDrawing({
			kind: 'text_note',
			start: { time: 100, price: 10 },
			text: 'supply zone',
			anchor: 'below'
		});
		expect(drawableFromDrawingRuntime(drawing)).toEqual({
			kind: 'label',
			ownerId: 'd1',
			at: { time: 100, value: 10 },
			text: 'supply zone',
			anchor: 'below',
			style: { textColor: '#7aa7e8', background: DEFAULT_LABEL_STYLE.background, fontSize: DEFAULT_LABEL_STYLE.fontSize }
		});
	});
});

describe('drawablesFromDrawingRuntime — the array entry point setDrawings uses', () => {
	test('every kind but fib_retracement is exactly the one object drawableFromDrawingRuntime already produces', () => {
		const drawing = baseDrawing({ kind: 'horizontal_line', price: 42_000 });
		expect(drawablesFromDrawingRuntime(drawing)).toEqual([drawableFromDrawingRuntime(drawing)]);
	});

	test('a fib retracement fans out to its six standard levels, interpolated between the two anchors', () => {
		const drawing = baseDrawing({
			kind: 'fib_retracement',
			start: { time: 0, price: 100 },
			end: { time: 0, price: 200 }
		});
		const levels = drawablesFromDrawingRuntime(drawing);
		expect(levels).toHaveLength(6);
		expect(levels.map((d) => (d.kind === 'level' ? d.price : null))).toEqual([100, 123.6, 138.2, 150, 161.8, 200]);
		for (const level of levels) {
			expect(level.kind).toBe('level');
			expect(level.ownerId).toBe('d1');
		}
	});

	test('a fib retracement drawn high-to-low still produces levels in the same 0%..100% order', () => {
		const drawing = baseDrawing({
			kind: 'fib_retracement',
			start: { time: 0, price: 200 },
			end: { time: 0, price: 100 }
		});
		const levels = drawablesFromDrawingRuntime(drawing);
		expect(levels.map((d) => (d.kind === 'level' ? d.price : null))).toEqual([200, 176.4, 161.8, 150, 138.2, 100]);
	});
});

describe('patchFromDrawable — the exact inverse, for persisting a drag', () => {
	test('a moved level patches back only price', () => {
		const drawable: ObjectDrawable = { kind: 'level', ownerId: 'd1', price: 55, extend: 'both', style: { color: '#fff', width: 1, lineStyle: 'SOLID' } };
		expect(patchFromDrawable(drawable)).toEqual({ price: 55 });
	});

	test('a moved segment patches back start/end at the drawing wire\'s field names', () => {
		const drawable: ObjectDrawable = {
			kind: 'segment',
			ownerId: 'd1',
			a: { time: 1, value: 2 },
			b: { time: 3, value: 4 },
			extend: 'none',
			style: { color: '#fff', width: 1, lineStyle: 'SOLID' }
		};
		expect(patchFromDrawable(drawable)).toEqual({ start: { time: 1, price: 2 }, end: { time: 3, price: 4 } });
	});

	test('a moved box patches back start/end the same way a segment does', () => {
		const drawable: ObjectDrawable = {
			kind: 'box',
			ownerId: 'd1',
			a: { time: 1, value: 2 },
			b: { time: 3, value: 4 },
			style: { color: '#fff', width: 1, lineStyle: 'SOLID', fillOpacity: 0.1 }
		};
		expect(patchFromDrawable(drawable)).toEqual({ start: { time: 1, price: 2 }, end: { time: 3, price: 4 } });
	});

	test('a moved label (a persisted text_note — the only label a drag can ever target) patches back its anchor as `start`', () => {
		const drawable: ObjectDrawable = {
			kind: 'label',
			ownerId: 'd1',
			at: { time: 1, value: 2 },
			text: 'supply zone',
			anchor: 'above',
			style: { textColor: '#000', background: '#fff', fontSize: 11 }
		};
		expect(patchFromDrawable(drawable)).toEqual({ start: { time: 1, price: 2 } });
	});
});
