import { describe, expect, test } from 'bun:test';
import { barChange, formatVolume, statusLineSegments, type StatusBar } from './status-line';

const bar: StatusBar = {
	open: 100,
	high: 110,
	low: 95,
	close: 105,
	previousClose: 100,
	volume: { kind: 'real', value: 12_345 }
};

const all = { statusOhlc: true, statusChange: true, statusVolume: true };
const space = { precision: 2, compact: false };

describe('the status line shows what the pane was told to show', () => {
	test('every group on gives OHLC, the change, and volume in reading order', () => {
		const segments = statusLineSegments(bar, all, space);
		expect(segments.map((s) => s.label)).toEqual(['O', 'H', 'L', 'C', '', 'VOL']);
		expect(segments[3].value).toBe('105.00');
	});

	test('a group the reader turned off is absent, not blank', () => {
		const segments = statusLineSegments(bar, { ...all, statusVolume: false }, space);
		expect(segments.some((s) => s.label === 'VOL')).toBe(false);
	});

	test('turning every group off leaves nothing at all — which is the point of the settings', () => {
		expect(
			statusLineSegments(bar, { statusOhlc: false, statusChange: false, statusVolume: false }, space)
		).toEqual([]);
	});

	test('a narrow pane keeps the close and drops the rest of the OHLC group', () => {
		const segments = statusLineSegments(bar, all, { ...space, compact: true });
		expect(segments.filter((s) => ['O', 'H', 'L', 'C'].includes(s.label)).map((s) => s.label)).toEqual(['C']);
	});

	test('prices are printed at the pane’s own precision, not a fixed one', () => {
		expect(statusLineSegments(bar, { ...all, statusChange: false, statusVolume: false }, { precision: 4, compact: true })[0].value).toBe('105.0000');
	});
});

describe('the change is measured against the previous close', () => {
	test('a rise is signed, tinted up, and carries its percentage', () => {
		const [change] = statusLineSegments(bar, { statusOhlc: false, statusChange: true, statusVolume: false }, space);
		expect(change.value).toBe('+5.00 (+5.00%)');
		expect(change.tone).toBe('up');
	});

	test('a fall is tinted down', () => {
		const falling = { ...bar, close: 95 };
		const [change] = statusLineSegments(falling, { statusOhlc: false, statusChange: true, statusVolume: false }, space);
		expect(change.tone).toBe('down');
		expect(change.value.startsWith('-')).toBe(true);
	});

	test('an unchanged close is neither up nor down', () => {
		const flat = { ...bar, close: 100 };
		const [change] = statusLineSegments(flat, { statusOhlc: false, statusChange: true, statusVolume: false }, space);
		expect(change.tone).toBe('neutral');
	});

	// The gap between yesterday's close and today's open is exactly what a
	// change column is supposed to include; measuring from this bar's own
	// open would silently drop it.
	test('it is the previous close, not this bar’s open, that the change is measured from', () => {
		const gapped: StatusBar = { ...bar, open: 104, close: 105, previousClose: 100 };
		expect(barChange(gapped)).toEqual({ absolute: 5, percent: 5 });
	});

	test('the first bar of a window has nothing to compare against and reports no change', () => {
		expect(barChange({ ...bar, previousClose: null })).toBeNull();
		expect(
			statusLineSegments({ ...bar, previousClose: null }, { statusOhlc: false, statusChange: true, statusVolume: false }, space)
		).toEqual([]);
	});

	test('a previous close of zero yields no percentage rather than an infinite one', () => {
		expect(barChange({ ...bar, previousClose: 0 })?.percent).toBeNull();
	});
});

describe('volume is labelled for what the venue actually reported', () => {
	test('a base-asset quantity is abbreviated and labelled VOL', () => {
		expect(formatVolume({ kind: 'real', value: 12_345 })).toBe('12.3K');
		expect(formatVolume({ kind: 'real', value: 2_500_000 })).toBe('2.50M');
		expect(formatVolume({ kind: 'real', value: 3_100_000_000 })).toBe('3.10B');
		expect(formatVolume({ kind: 'real', value: 812 })).toBe('812.00');
	});

	// A tick count is a number of price changes, not a quantity. Rounding it
	// to "1.2K" throws away the only thing it says, and calling it VOL says
	// something the venue did not.
	test('a tick count keeps its exact value and its own label', () => {
		expect(formatVolume({ kind: 'tick', value: 1234 })).toBe('1234');
		const [segment] = statusLineSegments(
			{ ...bar, volume: { kind: 'tick', value: 1234 } },
			{ statusOhlc: false, statusChange: false, statusVolume: true },
			space
		);
		expect(segment.label).toBe('TICKS');
		expect(segment.value).toBe('1234');
	});

	test('a venue that reports no volume shows no volume cell', () => {
		expect(
			statusLineSegments({ ...bar, volume: null }, { statusOhlc: false, statusChange: false, statusVolume: true }, space)
		).toEqual([]);
	});
});
