// Proves the property this widget exists for: an exact position size from
// exact inputs, computed on the digits — never through a `Number`/`f64` at
// any step — and a zero stop distance rejected with a message rather than
// producing `Infinity` or `NaN`.
import { describe, expect, test } from 'bun:test';
import { calculatePositionSize, formatExact, type PositionSizeFields } from './position-size-calc';

function fields(overrides: Partial<PositionSizeFields> = {}): PositionSizeFields {
	return {
		balance: '',
		riskPercent: '',
		entryPrice: '',
		stopPrice: '',
		sizeDecimals: '',
		...overrides
	};
}

describe('calculatePositionSize', () => {
	test('reports empty until every required field is filled in', () => {
		expect(calculatePositionSize(fields())).toEqual({ kind: 'empty' });
		expect(calculatePositionSize(fields({ balance: '10000' }))).toEqual({ kind: 'empty' });
		expect(
			calculatePositionSize(fields({ balance: '10000', riskPercent: '1', entryPrice: '100' }))
		).toEqual({ kind: 'empty' });
	});

	test('a textbook example computes the exact risk amount and size', () => {
		// $10,000 balance, 1% risk, entry 100, stop 95 -> $100 at risk over a
		// $5 stop distance -> 20 units, exactly, with no fractional drift.
		const outcome = calculatePositionSize(
			fields({
				balance: '10000',
				riskPercent: '1',
				entryPrice: '100',
				stopPrice: '95',
				sizeDecimals: '2'
			})
		);
		expect(outcome.kind).toBe('ok');
		if (outcome.kind !== 'ok') return;
		expect(formatExact(outcome.result.riskAmount)).toBe('100.00');
		expect(formatExact(outcome.result.stopDistance)).toBe('5');
		expect(formatExact(outcome.result.positionSize)).toBe('20.00');
	});

	test('stop below or above entry makes no difference — distance is absolute', () => {
		const stopBelow = calculatePositionSize(
			fields({ balance: '10000', riskPercent: '1', entryPrice: '100', stopPrice: '95' })
		);
		const stopAbove = calculatePositionSize(
			fields({ balance: '10000', riskPercent: '1', entryPrice: '95', stopPrice: '100' })
		);
		expect(stopBelow.kind).toBe('ok');
		expect(stopAbove.kind).toBe('ok');
		if (stopBelow.kind !== 'ok' || stopAbove.kind !== 'ok') return;
		expect(formatExact(stopBelow.result.positionSize)).toBe(
			formatExact(stopAbove.result.positionSize)
		);
	});

	test('a zero stop distance is rejected with a message, never Infinity or NaN', () => {
		const outcome = calculatePositionSize(
			fields({ balance: '10000', riskPercent: '1', entryPrice: '100', stopPrice: '100' })
		);
		expect(outcome.kind).toBe('error');
		if (outcome.kind !== 'error') return;
		expect(outcome.message).not.toContain('Infinity');
		expect(outcome.message).not.toContain('NaN');
		expect(outcome.message.length).toBeGreaterThan(0);
	});

	test('garbage input is rejected rather than silently parsed as zero', () => {
		const outcome = calculatePositionSize(
			fields({ balance: 'abc', riskPercent: '1', entryPrice: '100', stopPrice: '95' })
		);
		expect(outcome.kind).toBe('error');
	});

	test('a negative balance or risk percent is rejected, not computed against', () => {
		const outcome = calculatePositionSize(
			fields({ balance: '-10000', riskPercent: '1', entryPrice: '100', stopPrice: '95' })
		);
		expect(outcome.kind).toBe('error');
	});

	test('the result never loses precision the way a float division would', () => {
		// 1/3 style repeating division: this must truncate at the requested
		// scale rather than rounding through a double's own approximation.
		const outcome = calculatePositionSize(
			fields({
				balance: '100',
				riskPercent: '10',
				entryPrice: '100',
				stopPrice: '97',
				sizeDecimals: '6'
			})
		);
		expect(outcome.kind).toBe('ok');
		if (outcome.kind !== 'ok') return;
		// risk amount = 10.00, distance = 3, exact quotient is
		// 3.333333... truncated (never rounded up) at 6 places.
		expect(formatExact(outcome.result.positionSize)).toBe('3.333333');
	});

	test('an unset size-decimals field defaults to two places rather than rejecting', () => {
		const outcome = calculatePositionSize(
			fields({ balance: '10000', riskPercent: '1', entryPrice: '100', stopPrice: '95' })
		);
		expect(outcome.kind).toBe('ok');
		if (outcome.kind !== 'ok') return;
		expect(outcome.result.positionSize.scale).toBe(2);
	});

	test('a size-decimals value above the cap is clamped rather than trusted verbatim', () => {
		const outcome = calculatePositionSize(
			fields({
				balance: '10000',
				riskPercent: '1',
				entryPrice: '100',
				stopPrice: '95',
				sizeDecimals: '99'
			})
		);
		expect(outcome.kind).toBe('ok');
		if (outcome.kind !== 'ok') return;
		expect(outcome.result.positionSize.scale).toBe(8);
	});
});
