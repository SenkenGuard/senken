import { describe, expect, test } from 'bun:test';
import { formatMeasureLabel, measureStats } from './measure';

describe('measureStats — pure delta math for the measure tool', () => {
	test('reports the raw price delta and its percent of the start price', () => {
		const stats = measureStats({ time: 0, value: 100 }, { time: 0, value: 120 }, 3600);
		expect(stats.priceDelta).toBe(20);
		expect(stats.percent).toBeCloseTo(20, 6);
	});

	test('a move down reports a negative delta and a negative percent', () => {
		const stats = measureStats({ time: 0, value: 100 }, { time: 0, value: 80 }, 3600);
		expect(stats.priceDelta).toBe(-20);
		expect(stats.percent).toBeCloseTo(-20, 6);
	});

	test('swapping the two points flips the sign of every field but leaves the magnitudes equal', () => {
		const a = { time: 1_000, value: 50 };
		const b = { time: 4_600, value: 55 };
		const forward = measureStats(a, b, 3600);
		const backward = measureStats(b, a, 3600);
		expect(backward.priceDelta).toBeCloseTo(-forward.priceDelta, 6);
		expect(backward.seconds).toBe(-forward.seconds);
		expect(backward.bars).toBe(-forward.bars);
	});

	test('a zero start price reports 0% rather than dividing by zero', () => {
		const stats = measureStats({ time: 0, value: 0 }, { time: 0, value: 10 }, 3600);
		expect(stats.percent).toBe(0);
		expect(Number.isFinite(stats.percent)).toBe(true);
	});

	test('bars is the time delta divided by the bar step, rounded to the nearest whole bar', () => {
		const stats = measureStats({ time: 0, value: 1 }, { time: 3 * 3600 + 1700, value: 1 }, 3600);
		expect(stats.bars).toBe(3);
	});

	test('a zero bar step never divides by zero', () => {
		const stats = measureStats({ time: 0, value: 1 }, { time: 3600, value: 1 }, 0);
		expect(stats.bars).toBe(0);
	});
});

describe('formatMeasureLabel — the text painted next to the pointer', () => {
	test('a positive move carries an explicit + sign on both the price delta and the percent', () => {
		const label = formatMeasureLabel({ priceDelta: 20, percent: 20, seconds: 3600, bars: 1 });
		expect(label).toBe('+20.0000  +20.00%  1 bar');
	});

	test('a negative move keeps the sign the numbers already carry, with no doubled minus', () => {
		const label = formatMeasureLabel({ priceDelta: -20, percent: -20, seconds: -3600, bars: -1 });
		expect(label).toBe('-20.0000  -20.00%  -1 bar');
	});

	test('bar count pluralizes except at exactly 1 (either sign)', () => {
		expect(formatMeasureLabel({ priceDelta: 0, percent: 0, seconds: 0, bars: 0 }).endsWith('0 bars')).toBe(true);
		expect(formatMeasureLabel({ priceDelta: 0, percent: 0, seconds: 0, bars: 1 }).endsWith('1 bar')).toBe(true);
		expect(formatMeasureLabel({ priceDelta: 0, percent: 0, seconds: 0, bars: -1 }).endsWith('-1 bar')).toBe(true);
		expect(formatMeasureLabel({ priceDelta: 0, percent: 0, seconds: 0, bars: 2 }).endsWith('2 bars')).toBe(true);
	});
});
