import { describe, expect, test } from 'bun:test';
import type { IndicatorDrawableDto } from '$lib/api/types';
import { DEFAULT_BOX_FILL_OPACITY, DEFAULT_INDICATOR_STROKE_STYLE, DEFAULT_LABEL_STYLE } from './drawable';
import { objectDrawablesFromIndicatorDisplay } from './indicator-display';

// Wire shapes exactly as `response.display[i]` reports them
// (`crates/api/src/dto/indicators.rs`'s `IndicatorDrawableDto`).
const SERIES: IndicatorDrawableDto = {
	kind: 'series',
	field: 'value',
	shape: 'line',
	points: [{ ts_open: 60_000_000_000, value: 100.0 }]
};

const SEGMENT: IndicatorDrawableDto = {
	kind: 'segment',
	a: { time: 60_000_000_000, value: 98.5 },
	b: { time: 120_000_000_000, value: 101.25 },
	extend: 'forward'
};

const LEVEL_ANNOTATION: IndicatorDrawableDto = {
	kind: 'level',
	price: { kind: 'annotation', value: 101.5 },
	extend: 'both'
};

const LEVEL_EXECUTABLE: IndicatorDrawableDto = {
	kind: 'level',
	price: { kind: 'executable', value: { value: 10_005_000, scale: 6 } },
	extend: 'none'
};

const BOX: IndicatorDrawableDto = {
	kind: 'box',
	a: { time: 60_000_000_000, value: 98.5 },
	b: { time: 120_000_000_000, value: 101.25 }
};

const LABEL: IndicatorDrawableDto = {
	kind: 'label',
	at: { time: 60_000_000_000, value: 101.5 },
	text: 'pivot high',
	anchor: 'above'
};

describe('objectDrawablesFromIndicatorDisplay — the computed counterpart to drawableFromDrawingRuntime', () => {
	test('a series entry produces no object — it is rendered as a line/histogram series, never hit-tested', () => {
		expect(objectDrawablesFromIndicatorDisplay([SERIES], 'layer-1')).toEqual([]);
	});

	test('a segment converts nanosecond time to seconds and keeps its extend and owner id', () => {
		expect(objectDrawablesFromIndicatorDisplay([SEGMENT], 'layer-1')).toEqual([
			{
				kind: 'segment',
				ownerId: 'layer-1',
				a: { time: 60, value: 98.5 },
				b: { time: 120, value: 101.25 },
				extend: 'forward',
				style: DEFAULT_INDICATOR_STROKE_STYLE
			}
		]);
	});

	test('a level with an annotation price coordinate uses its value directly', () => {
		expect(objectDrawablesFromIndicatorDisplay([LEVEL_ANNOTATION], 'layer-1')).toEqual([
			{
				kind: 'level',
				ownerId: 'layer-1',
				price: 101.5,
				extend: 'both',
				style: DEFAULT_INDICATOR_STROKE_STYLE
			}
		]);
	});

	test('a level with an executable price coordinate divides the scaled integer back to a real number', () => {
		// 10_005_000 at scale 6 -> 10.005 — this is display placement only,
		// never fed back toward an order (see the module's own doc).
		expect(objectDrawablesFromIndicatorDisplay([LEVEL_EXECUTABLE], 'layer-1')).toEqual([
			{
				kind: 'level',
				ownerId: 'layer-1',
				price: 10.005,
				extend: 'none',
				style: DEFAULT_INDICATOR_STROKE_STYLE
			}
		]);
	});

	test('a box converts both corners and carries the shared fill opacity', () => {
		expect(objectDrawablesFromIndicatorDisplay([BOX], 'layer-2')).toEqual([
			{
				kind: 'box',
				ownerId: 'layer-2',
				a: { time: 60, value: 98.5 },
				b: { time: 120, value: 101.25 },
				style: { ...DEFAULT_INDICATOR_STROKE_STYLE, fillOpacity: DEFAULT_BOX_FILL_OPACITY }
			}
		]);
	});

	test('a label converts its anchor point and keeps its text and anchor side', () => {
		expect(objectDrawablesFromIndicatorDisplay([LABEL], 'layer-2')).toEqual([
			{
				kind: 'label',
				ownerId: 'layer-2',
				at: { time: 60, value: 101.5 },
				text: 'pivot high',
				anchor: 'above',
				style: DEFAULT_LABEL_STYLE
			}
		]);
	});

	test('a mixed display list keeps every non-series object in order, tagged with the same owner', () => {
		const result = objectDrawablesFromIndicatorDisplay([SERIES, SEGMENT, LEVEL_ANNOTATION, BOX, LABEL], 'layer-3');
		expect(result.map((o) => o.kind)).toEqual(['segment', 'level', 'box', 'label']);
		expect(result.every((o) => o.ownerId === 'layer-3')).toBe(true);
	});
});
