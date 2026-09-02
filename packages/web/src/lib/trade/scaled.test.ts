import { describe, expect, test } from 'bun:test';
import {
	formatScaled,
	parseDecimalInput,
	formatScaledForDisplay,
	formatSigned,
	isNegative,
	isZero,
	parseScaled,
	toneOf
} from './scaled';

describe('formatScaled', () => {
	test('places the decimal point by the scale', () => {
		expect(formatScaled({ scale: 2, value: '6842013' })).toBe('68420.13');
		expect(formatScaled({ scale: 8, value: '5000000000000' })).toBe('50000.00000000');
		expect(formatScaled({ scale: 0, value: '42' })).toBe('42');
	});

	test('pads a value shorter than its own scale', () => {
		expect(formatScaled({ scale: 8, value: '1' })).toBe('0.00000001');
		expect(formatScaled({ scale: 2, value: '5' })).toBe('0.05');
	});

	test('keeps the sign in front of the whole number', () => {
		expect(formatScaled({ scale: 2, value: '-150' })).toBe('-1.50');
		expect(formatScaled({ scale: 8, value: '-1' })).toBe('-0.00000001');
	});

	test('never routes a large value through a double', () => {
		// 2^53 + 1 — the first integer a double cannot represent. Reading
		// this through `Number()` would come back one too small.
		const exact = { scale: 0, value: '9007199254740993' };
		expect(formatScaled(exact)).toBe('9007199254740993');
	});

	test('renders an absent value as a dash rather than a zero', () => {
		// A position with no mark price has no profit figure; showing 0.00
		// would be a claim, and a wrong one.
		expect(formatScaled(null)).toBe('—');
		expect(formatScaled(undefined)).toBe('—');
	});
});

describe('formatScaledForDisplay', () => {
	test('groups thousands and trims the fraction', () => {
		expect(formatScaledForDisplay({ scale: 8, value: '5000000000000' })).toBe('50,000.00');
		expect(formatScaledForDisplay({ scale: 2, value: '6842013' })).toBe('68,420.13');
	});

	test('keeps two decimals even when the value has none', () => {
		expect(formatScaledForDisplay({ scale: 0, value: '42' })).toBe('42.00');
	});

	test('shows more precision when asked', () => {
		expect(formatScaledForDisplay({ scale: 8, value: '12345678' }, 8)).toBe('0.12345678');
	});

	test('groups a negative number without separating its sign', () => {
		expect(formatScaledForDisplay({ scale: 2, value: '-123456' })).toBe('-1,234.56');
	});
});

describe('formatSigned', () => {
	test('marks a gain explicitly and leaves a loss as it is', () => {
		expect(formatSigned({ scale: 2, value: '150' })).toBe('+1.50');
		expect(formatSigned({ scale: 2, value: '-150' })).toBe('-1.50');
		expect(formatSigned({ scale: 2, value: '0' })).toBe('+0.00');
	});
});

describe('sign helpers', () => {
	test('read the sign without parsing the number', () => {
		expect(isNegative({ scale: 2, value: '-1' })).toBe(true);
		expect(isNegative({ scale: 2, value: '1' })).toBe(false);
		expect(isNegative(null)).toBe(false);
	});

	test('detect zero at any scale', () => {
		expect(isZero({ scale: 8, value: '0' })).toBe(true);
		expect(isZero({ scale: 8, value: '00000' })).toBe(true);
		expect(isZero({ scale: 8, value: '1' })).toBe(false);
	});

	test('colour a zero profit as a gain, not a loss', () => {
		expect(toneOf({ scale: 2, value: '0' })).toBe('gain');
		expect(toneOf({ scale: 2, value: '-1' })).toBe('loss');
	});
});

describe('parseScaled', () => {
	test('scales the digits the user typed', () => {
		expect(parseScaled('68420.13', 2)).toEqual({ scale: 2, value: '6842013' });
		expect(parseScaled('0.25', 3)).toEqual({ scale: 3, value: '250' });
		expect(parseScaled('5', 2)).toEqual({ scale: 2, value: '500' });
	});

	test('refuses more precision than the scale allows rather than rounding', () => {
		// Rounding here is how an order goes out at the wrong size.
		expect(parseScaled('0.001', 2)).toBeNull();
		expect(parseScaled('1.005', 2)).toBeNull();
	});

	test('refuses anything that is not a plain decimal', () => {
		expect(parseScaled('', 2)).toBeNull();
		expect(parseScaled('1e-6', 8)).toBeNull();
		expect(parseScaled('1,5', 2)).toBeNull();
		expect(parseScaled('abc', 2)).toBeNull();
		expect(parseScaled('-', 2)).toBeNull();
	});

	test('handles a negative amount', () => {
		expect(parseScaled('-1.50', 2)).toEqual({ scale: 2, value: '-150' });
	});

	test('normalises a negative zero to a plain zero', () => {
		expect(parseScaled('-0.00', 2)).toEqual({ scale: 2, value: '0' });
	});

	test('round-trips through formatScaled', () => {
		for (const [text, scale] of [
			['68420.13', 2],
			['0.00000001', 8],
			['-1234.5678', 4]
		] as const) {
			expect(formatScaled(parseScaled(text, scale))).toBe(text);
		}
	});
});

describe('parseDecimalInput', () => {
	test('keeps exactly the precision the user typed', () => {
		expect(parseDecimalInput('0.25')).toEqual({ scale: 2, value: '25' });
		expect(parseDecimalInput('0.250')).toEqual({ scale: 3, value: '250' });
		expect(parseDecimalInput('68420')).toEqual({ scale: 0, value: '68420' });
	});

	test('refuses anything that is not a plain decimal', () => {
		expect(parseDecimalInput('')).toBeNull();
		expect(parseDecimalInput('1e5')).toBeNull();
		expect(parseDecimalInput('abc')).toBeNull();
	});
});
