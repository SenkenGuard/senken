import { describe, expect, test } from 'bun:test';
import { parsePaneSettings, paneSettingsToJson } from './pane-settings';
import { defaultChartSettings } from '$lib/mock/chart-settings';

describe("parsePaneSettings — a pane's settings text must never break the chart", () => {
	test('the documented empty case, "{}", resolves to every default', () => {
		expect(parsePaneSettings('{}')).toEqual(defaultChartSettings());
	});

	test('a partial object merges over the defaults rather than replacing them', () => {
		const result = parsePaneSettings('{"bodyUp":"#ff0000","precision":"2"}');
		expect(result.bodyUp).toBe('#ff0000');
		expect(result.precision).toBe('2');
		// Everything not mentioned keeps its default, proving this is a merge,
		// not a full replace that would leave every other field undefined.
		expect(result.wicks).toBe(defaultChartSettings().wicks);
		expect(result.scaleFont).toBe(defaultChartSettings().scaleFont);
	});

	test('malformed JSON falls back to defaults instead of throwing', () => {
		expect(() => parsePaneSettings('{not valid json')).not.toThrow();
		expect(parsePaneSettings('{not valid json')).toEqual(defaultChartSettings());
	});

	test('valid JSON that is not an object (an array, a bare number) also falls back to defaults', () => {
		expect(parsePaneSettings('[1,2,3]')).toEqual(defaultChartSettings());
		expect(parsePaneSettings('42')).toEqual(defaultChartSettings());
		expect(parsePaneSettings('null')).toEqual(defaultChartSettings());
	});

	test('an empty string (never valid JSON) falls back to defaults too', () => {
		expect(parsePaneSettings('')).toEqual(defaultChartSettings());
	});
});

describe('paneSettingsToJson round-trip', () => {
	test('a changed settings object survives being serialised and re-parsed', () => {
		const changed = { ...defaultChartSettings(), bodyUp: '#123456', precision: '4' as const, scaleFont: 12 };
		const roundTripped = parsePaneSettings(paneSettingsToJson(changed));
		expect(roundTripped).toEqual(changed);
	});
});
