import { describe, expect, test } from 'bun:test';
import { alertFormPrefill } from './alert-form-prefill';

describe('alertFormPrefill — the new-alert form must follow the pane, not remember it', () => {
	test('a closed form has nothing to prefill', () => {
		expect(alertFormPrefill(false, { instrument: 'okx-spot:BTCUSDT', timeframe: '1h' })).toBeNull();
	});

	test('an open form prefills exactly the pane it was opened from', () => {
		expect(alertFormPrefill(true, { instrument: 'okx-spot:BTCUSDT', timeframe: '1h' })).toEqual({
			instrument: 'okx-spot:BTCUSDT',
			timeframe: '1h'
		});
	});

	// This is the regression test for the actual bug: a component that reads
	// `paneInstrument` once into a `$state` initializer captures only the
	// value at that instant and goes stale the moment the pane's instrument
	// changes — exactly what a naive fix (and the wrong comment that used to
	// sit above it) looked like. This asserts the *recomputed* answer for a
	// pane that changed while the form is still open, proving the result
	// tracks the live pane rather than a remembered one.
	test('switching the pane\'s instrument while the form stays open changes the prefill', () => {
		const opened = alertFormPrefill(true, { instrument: 'binance-spot:BTCUSDT', timeframe: '1h' });
		const afterPaneSwitch = alertFormPrefill(true, { instrument: 'okx-spot:BTCUSDT', timeframe: '15m' });
		expect(opened).toEqual({ instrument: 'binance-spot:BTCUSDT', timeframe: '1h' });
		expect(afterPaneSwitch).toEqual({ instrument: 'okx-spot:BTCUSDT', timeframe: '15m' });
		expect(afterPaneSwitch).not.toEqual(opened);
	});

	test('closing the form again stops prefilling regardless of what the pane last showed', () => {
		expect(alertFormPrefill(false, { instrument: 'okx-spot:BTCUSDT', timeframe: '15m' })).toBeNull();
	});
});
