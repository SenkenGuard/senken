import { describe, expect, test } from 'bun:test';
import { OVERLAY_INSTRUMENT_PLOT, plotsForLayer } from './layer-style';

describe('plotsForLayer for an overlay_instrument layer', () => {
	test('styles under one line plot, keyed by OVERLAY_INSTRUMENT_PLOT', () => {
		const plots = plotsForLayer('layer-1', OVERLAY_INSTRUMENT_PLOT);
		expect(plots).toHaveLength(1);
		expect(plots[0]).toMatchObject({ field: 'value', type: 'line', visible: true });
	});

	test('defaults to a colour distinct from the candles\' own #f2f2ef, so a freshly added overlay is visible against them', () => {
		const [plot] = plotsForLayer('layer-1', OVERLAY_INSTRUMENT_PLOT);
		expect(plot.color).not.toBe('#f2f2ef');
	});
});
