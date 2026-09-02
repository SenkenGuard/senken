// Position-size arithmetic: balance, risk percent, entry price and stop
// price in, an exact position size out.
//
// This widget touches no market or account data at all — every number
// below came from what a user typed into a box, and none of it is money
// wired off an adapter. That does not relax the rule in `AGENTS.md`
// ("Money is exact; indicator values are not"): a position size is a
// quantity, the same category as a price or a balance, so it is never
// carried as an `f64` on the way from input to answer. Every value here is
// a `{ scale, value }` pair — `value` the unscaled digits, exact — and the
// only operations are on the digits themselves (`BigInt`), the same
// discipline `$lib/trade/scaled.ts` and `$lib/trade/view.ts` already hold
// server-reported money to.
//
// Division truncates toward zero rather than rounding, in both directions
// this module divides: a position size that came out smaller than the
// exact answer is merely conservative; one that came out larger risks more
// than the trader asked for.

import type { ScaledDto } from '$lib/api/types';
import { formatScaledForDisplay, parseDecimalInput } from '$lib/trade/scaled';

/** The four numbers a user can type, plus how many decimal places to size
 * the answer to — every field the raw text from its own input box, parsed
 * only when a calculation is actually attempted. */
export interface PositionSizeFields {
	balance: string;
	riskPercent: string;
	entryPrice: string;
	stopPrice: string;
	/** How many decimal places the position size is truncated to. There is
	 * no instrument here to read a real lot step from, so this is the one
	 * place the user states the rounding explicitly rather than the widget
	 * guessing it. Blank or invalid falls back to 2. */
	sizeDecimals: string;
}

/** What a calculation actually produced. */
export interface PositionSizeResult {
	/** The money at risk: `balance * riskPercent / 100`, truncated to at
	 * least cent precision. */
	riskAmount: ScaledDto;
	/** `|entryPrice - stopPrice|`, at the wider of the two inputs' own
	 * scales — never invented precision. */
	stopDistance: ScaledDto;
	/** `riskAmount / stopDistance`, truncated to `sizeDecimals` places —
	 * the answer this widget exists to produce. */
	positionSize: ScaledDto;
}

/** The three states this calculator's form can be in:
 * - `empty` — at least one required field has not been typed yet; show a
 *   waiting state, never a zero that could pass for an answer.
 * - `error` — every required field has something in it, but it does not
 *   describe a computable position (not a plain decimal, or a zero stop
 *   distance); show `message`, never `Infinity`/`NaN`.
 * - `ok` — a real answer. */
export type PositionSizeOutcome =
	| { kind: 'empty' }
	| { kind: 'error'; message: string }
	| { kind: 'ok'; result: PositionSizeResult };

const DEFAULT_SIZE_DECIMALS = 2;
const MAX_SIZE_DECIMALS = 8;

/** The digits of `value` re-expressed as a `bigint` at `scale` — widening
 * never loses anything, since it only ever adds trailing zeros. */
function widen(value: ScaledDto, scale: number): bigint {
	return BigInt(value.value) * 10n ** BigInt(scale - value.scale);
}

/** `a - b`, exact, at the wider of the two scales. */
function subScaled(a: ScaledDto, b: ScaledDto): ScaledDto {
	const scale = Math.max(a.scale, b.scale);
	return { scale, value: (widen(a, scale) - widen(b, scale)).toString() };
}

/** The absolute value of a scaled figure — a distance has no sign. */
function absScaled(value: ScaledDto): ScaledDto {
	return value.value.startsWith('-') ? { scale: value.scale, value: value.value.slice(1) } : value;
}

/** `a * b`, exact — scales add, the same rule `$lib/trade/view.ts`'s own
 * `mulScaled` uses for a position's notional. */
function mulScaled(a: ScaledDto, b: ScaledDto): ScaledDto {
	return { scale: a.scale + b.scale, value: (BigInt(a.value) * BigInt(b.value)).toString() };
}

/** Re-expresses `value` at `scale`, truncating toward zero when narrowing —
 * the conservative direction for a figure that feeds a position size. */
function rescaleTruncating(value: ScaledDto, scale: number): ScaledDto {
	const diff = scale - value.scale;
	if (diff === 0) return value;
	if (diff > 0) return { scale, value: (BigInt(value.value) * 10n ** BigInt(diff)).toString() };
	return { scale, value: (BigInt(value.value) / 10n ** BigInt(-diff)).toString() };
}

/** `a / b`, truncated toward zero at `resultScale`. `null` when `b` is
 * zero — the caller turns that into an explicit rejection, never a
 * division that would otherwise produce `Infinity`. */
function divScaledTruncating(a: ScaledDto, b: ScaledDto, resultScale: number): ScaledDto | null {
	const denominatorDigits = BigInt(b.value);
	if (denominatorDigits === 0n) return null;
	const numerator = BigInt(a.value) * 10n ** BigInt(resultScale + b.scale);
	const denominator = denominatorDigits * 10n ** BigInt(a.scale);
	return { scale: resultScale, value: (numerator / denominator).toString() };
}

/** `true` for a scaled value of exactly zero, negative or positive. */
function isZeroScaled(value: ScaledDto): boolean {
	return /^-?0*$/.test(value.value);
}

/** Parses `raw` as a non-negative plain decimal, or `null` if it is not one
 * — a balance, a price and a risk percent are never negative here. */
function parseNonNegative(raw: string): ScaledDto | null {
	const parsed = parseDecimalInput(raw);
	if (!parsed || parsed.value.startsWith('-')) return null;
	return parsed;
}

/** Clamps the user's own rounding choice to a sane range, defaulting a
 * blank or unparseable value to two places rather than rejecting the whole
 * calculation over a formatting field. */
function clampSizeDecimals(raw: string): number {
	const trimmed = raw.trim();
	if (!trimmed) return DEFAULT_SIZE_DECIMALS;
	const parsed = Number.parseInt(trimmed, 10);
	if (!Number.isFinite(parsed) || parsed < 0) return DEFAULT_SIZE_DECIMALS;
	return Math.min(MAX_SIZE_DECIMALS, parsed);
}

/** Computes a position size from `fields`, or reports why it cannot yet.
 * Pure: no market data, no account data, no I/O — the whole point of this
 * widget is that it needs none. */
export function calculatePositionSize(fields: PositionSizeFields): PositionSizeOutcome {
	const required = [fields.balance, fields.riskPercent, fields.entryPrice, fields.stopPrice];
	if (required.some((value) => value.trim() === '')) {
		return { kind: 'empty' };
	}

	const balance = parseNonNegative(fields.balance);
	const riskPercent = parseNonNegative(fields.riskPercent);
	const entryPrice = parseNonNegative(fields.entryPrice);
	const stopPrice = parseNonNegative(fields.stopPrice);
	if (!balance || !riskPercent || !entryPrice || !stopPrice) {
		return { kind: 'error', message: 'Enter plain, non-negative numbers only.' };
	}

	const stopDistance = absScaled(subScaled(entryPrice, stopPrice));
	if (isZeroScaled(stopDistance)) {
		return {
			kind: 'error',
			message: 'Entry and stop price cannot be equal — there is no distance to risk against.'
		};
	}

	// riskPercent / 100 as an exact ratio: shifting the scale by two places
	// divides by 100 without ever touching the digits, since
	// value / 10^(scale+2) is exactly (value / 10^scale) / 100.
	const riskRatio: ScaledDto = { scale: riskPercent.scale + 2, value: riskPercent.value };
	const riskAmountExact = mulScaled(balance, riskRatio);
	const moneyScale = Math.max(balance.scale, 2);
	const riskAmount = rescaleTruncating(riskAmountExact, moneyScale);

	const sizeDecimals = clampSizeDecimals(fields.sizeDecimals);
	const positionSize = divScaledTruncating(riskAmount, stopDistance, sizeDecimals);
	if (!positionSize) {
		// stopDistance was already proven non-zero above; this is
		// unreachable in practice and kept only so the type stays honest
		// about divScaledTruncating's own signature.
		return { kind: 'error', message: 'Stop distance must be greater than zero.' };
	}

	return { kind: 'ok', result: { riskAmount, stopDistance, positionSize } };
}

/** Renders a result field for display — full precision, grouped, never
 * trimmed below the value's own scale (unlike
 * `formatScaledForDisplay`'s two-decimal default). */
export function formatExact(value: ScaledDto): string {
	return formatScaledForDisplay(value, value.scale);
}
