import { describe, expect, test } from 'bun:test';
import { parsePaneSettings, paneSettingsToJson, shouldFramePriceScale } from './pane-settings';
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

describe('shouldFramePriceScale', () => {
	const state = { hasBars: true, alreadyFramed: false, storedAutoScale: false };

	test('frames a series whose first bars arrived after a stored manual scale', () => {
		// The reported bug: the axis was frozen while the series was still
		// empty, so the bars landed outside the visible range.
		expect(shouldFramePriceScale(state)).toBe(true);
	});

	test('does not frame a series that has no bars yet', () => {
		// Fitting an empty series only freezes a different meaningless range.
		expect(shouldFramePriceScale({ ...state, hasBars: false })).toBe(false);
	});

	test('does not frame a series twice', () => {
		// Otherwise a later reload of the same series would throw away a
		// range the reader dragged to during the session.
		expect(shouldFramePriceScale({ ...state, alreadyFramed: true })).toBe(false);
	});

	test('leaves a scale that is already auto alone', () => {
		// Writing a price-scale option that is already correct discards the
		// manual range, so there must be nothing to write here.
		expect(shouldFramePriceScale({ ...state, storedAutoScale: true })).toBe(false);
	});
});
