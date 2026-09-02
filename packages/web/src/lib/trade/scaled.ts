// Formatting and parsing the `{ scale, value }` pairs the trade engine
// speaks.
//
// The rule this module exists to hold: **a scaled value is never turned
// into a `number` on the way to the screen.** `value` arrives as a decimal
// string precisely so it does not go through a double, and `Number(...)`
// on it would undo that in one character — silently, and only for the
// large or precise values where it matters most.
//
// So everything here is string arithmetic. It is a small amount of code and
// it is exact.

import type { ScaledDto } from '$lib/api/types';

/** Renders `{ scale, value }` as a plain decimal string: `6842013` at scale
 * 2 becomes `68420.13`. Negative values keep their sign in front. */
export function formatScaled(scaled: ScaledDto | null | undefined): string {
	if (!scaled) return '—';
	const negative = scaled.value.startsWith('-');
	const digits = (negative ? scaled.value.slice(1) : scaled.value).padStart(scaled.scale + 1, '0');
	const whole = digits.slice(0, digits.length - scaled.scale) || '0';
	const fraction = scaled.scale > 0 ? digits.slice(digits.length - scaled.scale) : '';
	const sign = negative ? '-' : '';
	return fraction ? `${sign}${whole}.${fraction}` : `${sign}${whole}`;
}

/** As `formatScaled`, with thousands separators on the whole part and the
 * fraction trimmed to `maxFraction` digits — for a figure being read rather
 * than one being copied. Trailing zeros beyond two decimals are dropped, so
 * a balance at the simulator's eight-decimal cash scale reads `50,000.00`
 * rather than `50,000.00000000`. */
export function formatScaledForDisplay(
	scaled: ScaledDto | null | undefined,
	maxFraction = 2
): string {
	if (!scaled) return '—';
	const plain = formatScaled(scaled);
	const negative = plain.startsWith('-');
	const [whole, fraction = ''] = (negative ? plain.slice(1) : plain).split('.');
	const grouped = whole.replace(/\B(?=(\d{3})+(?!\d))/g, ',');
	const trimmed = fraction.slice(0, maxFraction).padEnd(Math.min(maxFraction, 2), '0');
	return `${negative ? '-' : ''}${grouped}${trimmed ? `.${trimmed}` : ''}`;
}

/** `formatScaledForDisplay` with an explicit `+` on non-negative values —
 * for a profit figure, where the sign is the point. */
export function formatSigned(scaled: ScaledDto | null | undefined, maxFraction = 2): string {
	if (!scaled) return '—';
	const rendered = formatScaledForDisplay(scaled, maxFraction);
	return rendered.startsWith('-') ? rendered : `+${rendered}`;
}

/** Whether a scaled value is negative — the one comparison a colour
 * decision needs, without parsing anything. */
export function isNegative(scaled: ScaledDto | null | undefined): boolean {
	return !!scaled && scaled.value.startsWith('-');
}

/** Whether a scaled value is exactly zero. */
export function isZero(scaled: ScaledDto | null | undefined): boolean {
	return !!scaled && /^-?0*$/.test(scaled.value);
}

/** `'gain'` for zero or positive, `'loss'` for negative — the tone every
 * profit figure in the trade UI is coloured by. */
export function toneOf(scaled: ScaledDto | null | undefined): 'gain' | 'loss' {
	return isNegative(scaled) ? 'loss' : 'gain';
}

/** Parses a decimal string the user typed into `{ scale, value }` at
 * exactly `scale`, or `null` if it is not a plain decimal or is finer than
 * `scale` allows.
 *
 * Refuses rather than rounds, the same rule the server applies: a size the
 * user typed that quietly loses a digit is how an order goes out at the
 * wrong quantity. */
export function parseScaled(raw: string, scale: number): ScaledDto | null {
	const text = raw.trim();
	if (!/^-?\d*(\.\d*)?$/.test(text) || text === '' || text === '-' || text === '.') return null;
	const negative = text.startsWith('-');
	const [whole, fraction = ''] = (negative ? text.slice(1) : text).split('.');
	if (fraction.length > scale) return null;
	const digits = `${whole || '0'}${fraction.padEnd(scale, '0')}`;
	// Strip leading zeros so two equal values have one representation, but
	// keep a lone `0`.
	const normalised = digits.replace(/^0+(?=\d)/, '');
	return { scale, value: `${negative && !/^0*$/.test(normalised) ? '-' : ''}${normalised}` };
}

/** Parses a decimal the user typed, keeping exactly the precision they
 * typed it with: `0.25` becomes `{ scale: 2, value: '25' }`.
 *
 * The scale comes from the input rather than from a schema because an order
 * ticket does not know an instrument's tick until the server tells it. The
 * server rounds down onto the real tick and step, so what matters here is
 * only that no digit is invented and none is lost. */
export function parseDecimalInput(raw: string): ScaledDto | null {
	const text = raw.trim();
	const fraction = text.includes('.') ? text.split('.')[1] : '';
	return parseScaled(text, fraction.length);
}

/** A scaled zero at `scale`. */
export function zeroAt(scale: number): ScaledDto {
	return { scale, value: '0' };
}
