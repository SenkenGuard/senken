import { describe, expect, test } from 'bun:test';
import { DRAWING_KIND_FOR_TOOL, TOOL_ANCHOR_COUNT, drawingFromDto, drawingToInput, objectTree, type DrawingRuntime } from './pane-runtime';
import type { DrawingDto } from '$lib/api/types';

function baseDto(overrides: Partial<DrawingDto>): DrawingDto {
	return {
		id: 'd1',
		position: 0,
		visible: true,
		color: '#7aa7e8',
		width: 2,
		line_style: 'SOLID',
		kind: { kind: 'horizontal_line', price: 100 },
		...overrides
	};
}

describe('drawingFromDto — new schema-v8 kinds round-trip through drawingToInput', () => {
	test('a horizontal_line carries the server\'s real `visible`, not a hardcoded default', () => {
		const hidden = drawingFromDto(baseDto({ visible: false }));
		expect(hidden.visible).toBe(false);
		const shown = drawingFromDto(baseDto({ visible: true }));
		expect(shown.visible).toBe(true);
	});

	test('a ray reads start/end and converts wire nanoseconds to chart seconds', () => {
		const dto = baseDto({
			kind: { kind: 'ray', start: { time: 60_000_000_000, price: 10 }, end: { time: 120_000_000_000, price: 20 } }
		});
		const runtime = drawingFromDto(dto);
		expect(runtime).toMatchObject({
			kind: 'ray',
			start: { time: 60, price: 10 },
			end: { time: 120, price: 20 }
		});
	});

	test('a fib_retracement reads start/end the same way a ray does', () => {
		const dto = baseDto({
			kind: { kind: 'fib_retracement', start: { time: 60_000_000_000, price: 100 }, end: { time: 120_000_000_000, price: 200 } }
		});
		const runtime = drawingFromDto(dto);
		expect(runtime).toMatchObject({
			kind: 'fib_retracement',
			start: { time: 60, price: 100 },
			end: { time: 120, price: 200 }
		});
	});

	test('a text_note reads `at` into `start`, plus its text and anchor', () => {
		const dto = baseDto({
			kind: { kind: 'text_note', at: { time: 60_000_000_000, price: 42 }, text: 'supply zone', anchor: 'below' }
		});
		const runtime = drawingFromDto(dto);
		expect(runtime).toMatchObject({
			kind: 'text_note',
			start: { time: 60, price: 42 },
			text: 'supply zone',
			anchor: 'below'
		});
	});

	test('every new kind survives a round trip back through drawingToInput', () => {
		const dtos: DrawingDto[] = [
			baseDto({ kind: { kind: 'ray', start: { time: 60_000_000_000, price: 10 }, end: { time: 120_000_000_000, price: 20 } } }),
			baseDto({
				kind: { kind: 'fib_retracement', start: { time: 60_000_000_000, price: 100 }, end: { time: 120_000_000_000, price: 200 } }
			}),
			baseDto({ kind: { kind: 'text_note', at: { time: 60_000_000_000, price: 42 }, text: 'note', anchor: 'center' }, visible: false })
		];
		for (const dto of dtos) {
			const input = drawingToInput(drawingFromDto(dto));
			expect(input.kind).toEqual(dto.kind);
			expect(input.visible).toBe(dto.visible);
		}
	});
});

describe('objectTree — a drawing group uses each drawing\'s own `visible`, not a hardcoded true', () => {
	function drawing(overrides: Partial<DrawingRuntime>): DrawingRuntime {
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

	test('a hidden drawing reports visible: false in the DRAWINGS group', () => {
		const groups = objectTree([], [drawing({ visible: false })]);
		const drawings = groups.find((g) => g.title === 'DRAWINGS');
		expect(drawings?.items).toEqual([{ name: 'HORIZONTAL LINE', params: '100.00', visible: false }]);
	});

	test('a shown drawing still reports visible: true — proving this is the real field, not always-false either', () => {
		const groups = objectTree([], [drawing({ visible: true })]);
		const drawings = groups.find((g) => g.title === 'DRAWINGS');
		expect(drawings?.items[0]?.visible).toBe(true);
	});
});

describe('the new tools are wired through the same data tables as the original three', () => {
	test('ray, fib_retracement and text_note each have an anchor count and a DrawingKindTag', () => {
		expect(TOOL_ANCHOR_COUNT.ray).toBe(2);
		expect(TOOL_ANCHOR_COUNT.fib).toBe(2);
		expect(TOOL_ANCHOR_COUNT.text).toBe(1);
		expect(DRAWING_KIND_FOR_TOOL.ray).toBe('ray');
		expect(DRAWING_KIND_FOR_TOOL.fib).toBe('fib_retracement');
		expect(DRAWING_KIND_FOR_TOOL.text).toBe('text_note');
	});
});
