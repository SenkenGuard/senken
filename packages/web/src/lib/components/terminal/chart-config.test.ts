import { describe, expect, test } from 'bun:test';
import {
	DRAW_TOOLS,
	OBJECT_DRAW_TOOLS,
	isIntradaySpec,
	timeAxisFormatter,
	timeLabelOptionsSignature,
	buildTimeLabelOptions
} from './chart-config';
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

describe('timeAxisFormatter — the axis/crosshair label follows the viewer\'s zone, never the instant', () => {
	// 20:00:00Z on 2026-09-01 is 03:00:00 on 2026-09-02 in Asia/Jakarta
	// (UTC+7, no DST) — deliberately near a day boundary so a *date* label
	// (the non-intraday branch) is also proven to move with the zone, not
	// only a clock.
	const SECONDS = Date.UTC(2026, 8, 1, 20, 0, 0) / 1000;

	test('an intraday spec renders a different clock in a different zone, for the same bar time', () => {
		const utc = timeAxisFormatter('1h', false, '24H', 'UTC')(SECONDS);
		const jakarta = timeAxisFormatter('1h', false, '24H', 'Asia/Jakarta')(SECONDS);
		expect(utc).toBe('20:00');
		expect(jakarta).toBe('03:00');
	});

	test('a daily spec renders a different calendar date in a different zone, for the same bar time', () => {
		const utc = timeAxisFormatter('1d', false, '24H', 'UTC')(SECONDS);
		const jakarta = timeAxisFormatter('1d', false, '24H', 'Asia/Jakarta')(SECONDS);
		expect(utc).toBe('Sep 1');
		expect(jakarta).toBe('Sep 2');
	});

	test('12H vs 24H is independent of zone', () => {
		const h24 = timeAxisFormatter('1h', false, '24H', 'UTC')(SECONDS);
		const h12 = timeAxisFormatter('1h', false, '12H', 'UTC')(SECONDS);
		expect(h24).toBe('20:00');
		expect(h12).toMatch(/^0?8:00\s?PM$/);
	});
});

describe('timeLabelOptionsSignature — the guard that skips a needless chart.applyOptions', () => {
	test('identical inputs produce identical signatures', () => {
		const a = timeLabelOptionsSignature('1h', true, '24H', 'Asia/Jakarta');
		const b = timeLabelOptionsSignature('1h', true, '24H', 'Asia/Jakarta');
		expect(a).toBe(b);
	});

	test('a zone change alone changes the signature — this is what makes a zone change actually reach the chart', () => {
		const before = timeLabelOptionsSignature('1h', true, '24H', 'UTC');
		const after = timeLabelOptionsSignature('1h', true, '24H', 'Asia/Jakarta');
		expect(before).not.toBe(after);
	});

	test('every other input held fixed but the zone changes independently of spec/dayOfWeek/timeFormat', () => {
		const base = timeLabelOptionsSignature('1d', false, '12H', 'UTC');
		expect(timeLabelOptionsSignature('1d', false, '12H', 'UTC')).toBe(base);
		expect(timeLabelOptionsSignature('4h', false, '12H', 'UTC')).not.toBe(base);
		expect(timeLabelOptionsSignature('1d', true, '12H', 'UTC')).not.toBe(base);
		expect(timeLabelOptionsSignature('1d', false, '24H', 'UTC')).not.toBe(base);
	});
});

describe('buildTimeLabelOptions — proof that a zone-driven label update cannot also touch the viewport', () => {
	// The dangerous failure mode this guards: an effect that re-runs when the
	// zone changes reaching for `timeScale.rightOffset` or a visible-range
	// field the way `chart-pane.svelte`'s own `applyTheme` history once did,
	// discarding wherever the reader had scrolled to. `buildTimeLabelOptions`'s
	// return type (`TimeLabelOptions`) only ever has room for the two label
	// fields — this test proves that structurally, by whitelisting every key
	// the real object actually carries at runtime, not merely by reading the
	// type.
	test('the built options carry only the two label-formatting fields — nothing viewport-shaped', () => {
		const options = buildTimeLabelOptions('1h', false, '24H', 'Asia/Jakarta');
		expect(Object.keys(options).sort()).toEqual(['localization', 'timeScale']);
		expect(Object.keys(options.timeScale)).toEqual(['tickMarkFormatter']);
		expect(Object.keys(options.localization)).toEqual(['timeFormatter']);
	});

	test('the axis formatter and the crosshair formatter are the same function — an axis tick and a hovered candle must always agree', () => {
		const options = buildTimeLabelOptions('1h', true, '12H', 'Asia/Jakarta');
		expect(options.timeScale.tickMarkFormatter).toBe(options.localization.timeFormatter);
	});

	test('changing only the zone changes the rendered label text, never the bar time the formatter is called with', () => {
		const utc = buildTimeLabelOptions('1h', false, '24H', 'UTC');
		const jakarta = buildTimeLabelOptions('1h', false, '24H', 'Asia/Jakarta');
		const seconds = Date.UTC(2026, 8, 1, 20, 0, 0) / 1000;
		expect(utc.timeScale.tickMarkFormatter(seconds)).toBe('20:00');
		expect(jakarta.timeScale.tickMarkFormatter(seconds)).toBe('03:00');
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
