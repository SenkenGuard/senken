import { describe, expect, test } from 'bun:test';
import { detectBrowserZone } from './zone';

describe('detectBrowserZone', () => {
	test('returns a non-empty zone id', () => {
		// This is a *default proposal*, not a source of truth — see this
		// module's own docs — so the only property worth pinning here is
		// that it returns something usable at all, not any particular zone
		// (the runtime running this test could be configured with any zone).
		const zone = detectBrowserZone();
		expect(typeof zone).toBe('string');
		expect(zone.length).toBeGreaterThan(0);
		// A real IANA id always resolves against `Intl` itself — this would
		// throw `RangeError` for a garbage value.
		expect(() => new Intl.DateTimeFormat(undefined, { timeZone: zone })).not.toThrow();
	});
});
