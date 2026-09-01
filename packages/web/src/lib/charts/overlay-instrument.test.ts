import { describe, expect, test } from 'bun:test';
import {
	overlayInstrumentCloseSeries,
	overlayInstrumentPriceScaleId,
	overlayInstrumentSignature,
	overlaySeriesKeysToRemove,
	planOverlayInstrumentSeries
} from './overlay-instrument';
import type { LayerRuntime } from './pane-runtime';

function layer(patch: Partial<LayerRuntime>): LayerRuntime {
	return {
		id: 'layer-1',
		position: 0,
		kind: 'overlay_instrument',
		visible: true,
		params: {},
		...patch
	};
}

describe('planOverlayInstrumentSeries', () => {
	test('plans one entry per overlay_instrument layer, skipping indicator layers and instrument-less ones', () => {
		const layers: LayerRuntime[] = [
			layer({ id: 'a', instrument: 'BINANCE:ETHUSDT' }),
			layer({ id: 'b', kind: 'indicator_overlay', indicatorName: 'Ema', instrument: undefined }),
			// An `overlay_instrument` layer somehow missing its instrument is
			// not plottable — must not produce a plan entry either.
			layer({ id: 'c', instrument: undefined })
		];
		const plan = planOverlayInstrumentSeries(layers);
		expect(plan.map((p) => p.layerId)).toEqual(['a']);
		expect(plan[0].instrument).toBe('BINANCE:ETHUSDT');
	});

	test('a hidden overlay layer still gets a plan entry, just marked not visible', () => {
		const layers: LayerRuntime[] = [layer({ id: 'a', instrument: 'BINANCE:ETHUSDT', visible: false })];
		const plan = planOverlayInstrumentSeries(layers);
		expect(plan).toHaveLength(1);
		expect(plan[0].visible).toBe(false);
	});

	test('two overlays get two distinct price-scale ids', () => {
		const layers: LayerRuntime[] = [
			layer({ id: 'a', instrument: 'BINANCE:ETHUSDT' }),
			layer({ id: 'b', instrument: 'BINANCE:SOLUSDT' })
		];
		const plan = planOverlayInstrumentSeries(layers);
		expect(plan.map((p) => p.priceScaleId)).toEqual([
			overlayInstrumentPriceScaleId('a'),
			overlayInstrumentPriceScaleId('b')
		]);
		expect(plan[0].priceScaleId).not.toBe(plan[1].priceScaleId);
	});
});

describe('overlaySeriesKeysToRemove', () => {
	test('a removed layer\'s series is dropped', () => {
		const stillThere = planOverlayInstrumentSeries([layer({ id: 'a', instrument: 'BINANCE:ETHUSDT' })]);
		// The caller previously held series for both 'a' and 'b'; 'b' has since
		// been removed from the layer set entirely (not just hidden).
		expect(overlaySeriesKeysToRemove(stillThere, ['a', 'b'])).toEqual(['b']);
	});

	test('a hidden-but-still-present layer is not queued for removal', () => {
		const plan = planOverlayInstrumentSeries([layer({ id: 'a', instrument: 'BINANCE:ETHUSDT', visible: false })]);
		expect(overlaySeriesKeysToRemove(plan, ['a'])).toEqual([]);
	});
});

describe('overlayInstrumentSignature', () => {
	test('changes when a layer is added, removed, or toggled, not on an unrelated re-render', () => {
		const base = [layer({ id: 'a', instrument: 'BINANCE:ETHUSDT' })];
		const same = [layer({ id: 'a', instrument: 'BINANCE:ETHUSDT' })];
		const hidden = [layer({ id: 'a', instrument: 'BINANCE:ETHUSDT', visible: false })];
		const removed: LayerRuntime[] = [];
		expect(overlayInstrumentSignature(base)).toBe(overlayInstrumentSignature(same));
		expect(overlayInstrumentSignature(base)).not.toBe(overlayInstrumentSignature(hidden));
		expect(overlayInstrumentSignature(base)).not.toBe(overlayInstrumentSignature(removed));
	});
});

describe('overlayInstrumentCloseSeries', () => {
	test('converts raw scaled-integer closes into decimal points at the given price scale', () => {
		const bars = [
			{ ts_open: 1_000_000_000, close: 123_456 },
			{ ts_open: 2_000_000_000, close: 123_789 }
		];
		expect(overlayInstrumentCloseSeries(bars, 2)).toEqual([
			{ time: 1, value: 1234.56 },
			{ time: 2, value: 1237.89 }
		]);
	});

	test("uses the overlay instrument's own price scale, not the main pane's — the two differ", () => {
		const bars = [{ ts_open: 0, close: 9_000_000 }];
		// The overlay instrument's own response says scale 6 (e.g. a low-priced
		// altcoin quoted to six decimals); the main pane's candles happen to be
		// at scale 2 (e.g. BTC quoted to cents). Applying the pane's own scale
		// instead of the overlay's is exactly the bug this function exists to
		// prevent.
		const overlayScale = 6;
		const paneScale = 2;
		const withOverlayScale = overlayInstrumentCloseSeries(bars, overlayScale);
		const withPaneScale = overlayInstrumentCloseSeries(bars, paneScale);
		expect(withOverlayScale[0].value).toBe(9);
		expect(withPaneScale[0].value).toBe(90_000);
		expect(withOverlayScale[0].value).not.toBe(withPaneScale[0].value);
	});
});
