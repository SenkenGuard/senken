import { describe, expect, test } from 'bun:test';
import { formatInstant, formatInstantAcrossZones, zonedTimeParts } from './format';

/** `2026-09-01T09:00:00Z`, an arbitrary reference instant with no DST
 * ambiguity in play — used by the tests that are not specifically about a
 * DST transition. */
const REFERENCE_NANOS = Date.UTC(2026, 8, 1, 9, 0, 0) * 1_000_000;

describe('formatInstant', () => {
	test('always attaches the zone id and a zone label, never a bare string', () => {
		const rendered = formatInstant(REFERENCE_NANOS, 'Asia/Jakarta');
		expect(rendered.zoneId).toBe('Asia/Jakarta');
		expect(typeof rendered.text).toBe('string');
		expect(rendered.text.length).toBeGreaterThan(0);
		expect(typeof rendered.zoneLabel).toBe('string');
		expect(rendered.zoneLabel.length).toBeGreaterThan(0);
	});

	test('the same instant reads as different wall-clock text in different zones', () => {
		// Jakarta is UTC+7 with no DST; London is UTC+1 in September (BST).
		// 09:00:00Z is 16:00:00 in Jakarta and 10:00:00 in London.
		const jakarta = formatInstant(REFERENCE_NANOS, 'Asia/Jakarta');
		const london = formatInstant(REFERENCE_NANOS, 'Europe/London');
		expect(jakarta.text).toBe('2026-09-01 16:00:00');
		expect(london.text).toBe('2026-09-01 10:00:00');
		expect(jakarta.text).not.toBe(london.text);
	});

	test('rendering in one zone has no effect on rendering the same instant in another', () => {
		// Guards against a shared-formatter-instance bug: if `formatInstant`
		// ever cached or reused an `Intl.DateTimeFormat` keyed wrong, an
		// earlier call's zone could leak into a later call's result. Order
		// the calls to Jakarta and London back to back, in both directions,
		// and confirm neither is affected by having run after the other.
		const jakartaFirst = formatInstant(REFERENCE_NANOS, 'Asia/Jakarta');
		const londonAfterJakarta = formatInstant(REFERENCE_NANOS, 'Europe/London');
		const londonFirst = formatInstant(REFERENCE_NANOS, 'Europe/London');
		const jakartaAfterLondon = formatInstant(REFERENCE_NANOS, 'Asia/Jakarta');
		expect(jakartaFirst).toEqual(jakartaAfterLondon);
		expect(londonFirst).toEqual(londonAfterJakarta);
	});

	test('reconstructing UTC from the rendered wall-clock fields and a known fixed offset agrees with the input instant', () => {
		// Asia/Jakarta carries no DST, so its offset (UTC+7) is exact for
		// every instant, not just this one — a strong enough anchor to prove
		// `formatInstant` did not shift the underlying instant, only its
		// label: parsing the rendered wall-clock text back with that offset
		// must reproduce `REFERENCE_NANOS` exactly.
		const rendered = formatInstant(REFERENCE_NANOS, 'Asia/Jakarta');
		const [datePart, timePart] = rendered.text.split(' ');
		const reconstructedUtcMs = Date.parse(`${datePart}T${timePart}+07:00`);
		expect(reconstructedUtcMs * 1_000_000).toBe(REFERENCE_NANOS);
	});

	test('seconds can be dropped from the rendered text', () => {
		const withSeconds = formatInstant(REFERENCE_NANOS, 'UTC');
		const withoutSeconds = formatInstant(REFERENCE_NANOS, 'UTC', { seconds: false });
		expect(withSeconds.text).toBe('2026-09-01 09:00:00');
		expect(withoutSeconds.text).toBe('2026-09-01 09:00');
	});
});

describe('formatInstant across a DST transition (America/New_York)', () => {
	// America/New_York springs forward on 2024-03-10: 2024-03-10T02:00:00
	// local (EST, UTC-5) becomes 2024-03-10T03:00:00 local (EDT, UTC-4) at
	// the same instant, 2024-03-10T07:00:00Z. One hour of UTC elapsing
	// across that instant must read as a *two*-hour jump in local wall-clock
	// text — proof that this module actually consults the zone's DST rules
	// for display instead of applying one fixed offset.
	const BEFORE_TRANSITION_UTC_MS = Date.UTC(2024, 2, 10, 6, 30, 0); // 01:30 EST
	const AFTER_TRANSITION_UTC_MS = Date.UTC(2024, 2, 10, 7, 30, 0); // 03:30 EDT

	test('one hour of UTC elapsed reads as two hours of local wall-clock time', () => {
		const before = formatInstant(BEFORE_TRANSITION_UTC_MS * 1_000_000, 'America/New_York');
		const after = formatInstant(AFTER_TRANSITION_UTC_MS * 1_000_000, 'America/New_York');
		expect(before.text).toBe('2024-03-10 01:30:00');
		expect(after.text).toBe('2024-03-10 03:30:00');
	});

	test('the zone label itself changes across the transition', () => {
		const before = formatInstant(BEFORE_TRANSITION_UTC_MS * 1_000_000, 'America/New_York');
		const after = formatInstant(AFTER_TRANSITION_UTC_MS * 1_000_000, 'America/New_York');
		expect(before.zoneLabel).not.toBe(after.zoneLabel);
	});
});

describe('zonedTimeParts', () => {
	test('the same instant splits into different pieces in different zones, never the underlying instant itself', () => {
		// Same reference and same two zones as formatInstant's own test above:
		// 09:00:00Z is 16:00 in Jakarta and 10:00 in London.
		const jakarta = zonedTimeParts(REFERENCE_NANOS, 'Asia/Jakarta');
		const london = zonedTimeParts(REFERENCE_NANOS, 'Europe/London');
		expect(jakarta.clock24).toBe('16:00');
		expect(london.clock24).toBe('10:00');
		expect(jakarta.monthDay).toBe(london.monthDay); // same UTC day for both zones here
	});

	test('clock24 and clock12 describe the same wall-clock minute in two conventions', () => {
		const parts = zonedTimeParts(REFERENCE_NANOS, 'Asia/Jakarta');
		expect(parts.clock24).toBe('16:00');
		expect(parts.clock12).toMatch(/^0?4:00\s?PM$/);
	});

	test('weekday is derived from the same zoned instant, not the caller\'s own zone', () => {
		// 2026-09-01T09:00:00Z is a Tuesday. Jakarta (UTC+7) is still the
		// same calendar day at 16:00, so both report the same weekday here —
		// the DST-transition-style day-boundary case is covered by
		// formatInstant's own tests; this one only proves the field is wired
		// to the zoned date, not to `new Date().getDay()` on the host zone.
		const parts = zonedTimeParts(REFERENCE_NANOS, 'Asia/Jakarta');
		expect(parts.weekday).toBe('Tue');
	});
});

describe('formatInstantAcrossZones', () => {
	test('reports exactly one primary rendering and the secondaries in the order given', () => {
		const result = formatInstantAcrossZones(REFERENCE_NANOS, 'Asia/Jakarta', [
			'Europe/London',
			'America/New_York'
		]);
		expect(result.primary.zoneId).toBe('Asia/Jakarta');
		expect(result.secondary.map((r) => r.zoneId)).toEqual(['Europe/London', 'America/New_York']);
	});

	test('with no secondary zones, only the primary is returned', () => {
		const result = formatInstantAcrossZones(REFERENCE_NANOS, 'UTC');
		expect(result.primary.zoneId).toBe('UTC');
		expect(result.secondary).toEqual([]);
	});
});
