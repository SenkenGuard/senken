import { describe, expect, test } from 'bun:test';
import { DRAW_TOOLS, OBJECT_DRAW_TOOLS, isIntradaySpec } from './chart-config';
import { DRAWING_KIND_FOR_TOOL, TOOL_ANCHOR_COUNT } from '$lib/charts/pane-runtime';

describe("isIntradaySpec — drives the time axis' timeVisible/secondsVisible", () => {
	test('every sub-daily spec is intraday', () => {
		for (const spec of ['1m', '5m', '15m', '1h', '4h']) {
			expect(isIntradaySpec(spec)).toBe(true);
		}
	});

	test('1d and 1w are not intraday — a date is the informative label there', () => {
		expect(isIntradaySpec('1d')).toBe(false);
		expect(isIntradaySpec('1w')).toBe(false);
	});

	test('an unrecognised spec defaults to intraday, matching every other TF_DURATION_SECONDS fallback in this file', () => {
		expect(isIntradaySpec('30s')).toBe(true);
	});
});

describe('drawing tool catalogue', () => {
	test('every selectable drawing tool is live in TOOL_ANCHOR_COUNT — none is a menu entry that does nothing when picked', () => {
		const selectableDrawingTools = DRAW_TOOLS.filter((tool) => tool.key !== 'cursor' && tool.key !== 'cross').map((tool) => tool.key);
		const liveTools = selectableDrawingTools.filter((key) => TOOL_ANCHOR_COUNT[key] != null);
		expect(liveTools.sort()).toEqual(selectableDrawingTools.sort());
	});

	test('OBJECT_DRAW_TOOLS matches DRAWING_KIND_FOR_TOOL exactly — every tool with a DrawingKindTag is listed there, and no other', () => {
		expect(Object.keys(DRAWING_KIND_FOR_TOOL).sort()).toEqual([...OBJECT_DRAW_TOOLS].sort());
	});

	test("the one live tool with no DrawingKindTag is measure — a transient reading, not a persisted object (see $lib/charts/measure.ts)", () => {
		const selectableDrawingTools = DRAW_TOOLS.filter((tool) => tool.key !== 'cursor' && tool.key !== 'cross').map((tool) => tool.key);
		const transientTools = selectableDrawingTools.filter((key) => !OBJECT_DRAW_TOOLS.includes(key));
		expect(transientTools).toEqual(['measure']);
		for (const key of transientTools) {
			expect(DRAWING_KIND_FOR_TOOL[key]).toBeUndefined();
			expect(TOOL_ANCHOR_COUNT[key]).toBeDefined();
		}
	});
});
