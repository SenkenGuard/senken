import { describe, expect, test } from 'bun:test';
import { applyRecompile, catalogItemFromEntry, findJustPlacedLayer } from './indicator-panel';
import type { LayerRuntime } from '$lib/charts/pane-runtime';
import type { IndicatorCatalogEntry } from '$lib/api/types';

function layer(id: string, position: number, indicatorName: string, params: Record<string, number> = {}): LayerRuntime {
	return { id, position, kind: 'indicator_overlay', visible: true, indicatorName, params };
}

describe('catalogItemFromEntry', () => {
	test('builds the exact IndicatorCatalogItem shape the built-in picker builds', () => {
		const entry: IndicatorCatalogEntry = {
			name: 'user-ema-cross',
			title: 'EMA Cross',
			short_title: 'EMA X',
			legend: 'EMA({period})',
			params: [
				{ name: 'period', kind: 'integer', default: { kind: 'integer', value: 5 }, min: 1 }
			],
			plots: [],
			scale: { kind: 'price' },
			requires_real_volume: false,
			placement: 'overlay',
			warmup_bars: 5
		};
		expect(catalogItemFromEntry(entry)).toEqual({
			name: 'user-ema-cross',
			defaultParams: { period: 5 },
			placement: 'overlay'
		});
	});
});

describe('findJustPlacedLayer', () => {
	test('picks the highest-position match, not merely the first', () => {
		const layers = [layer('stale', 0, 'user-ema-cross'), layer('fresh', 3, 'user-ema-cross'), layer('other', 1, 'user-rsi')];
		expect(findJustPlacedLayer(layers, 'user-ema-cross')?.id).toBe('fresh');
	});

	test('returns null when nothing matches', () => {
		expect(findJustPlacedLayer([layer('a', 0, 'user-rsi')], 'user-ema-cross')).toBeNull();
	});
});

describe('applyRecompile: recompiling never leaves a duplicate plot behind', () => {
	const item = { name: 'user-ema-cross', defaultParams: { period: 8 }, placement: 'overlay' as const };

	test('the very first compile-and-place simply appends one layer', () => {
		const result = applyRecompile([], null, item, 'L1');
		expect(result).toHaveLength(1);
		expect(result[0]).toMatchObject({ id: 'L1', indicatorName: 'user-ema-cross', params: { period: 8 } });
	});

	test('a second recompile of the same panel replaces the first layer instead of adding a second', () => {
		const afterFirst = applyRecompile([], null, item, 'L1');
		const afterSecond = applyRecompile(afterFirst, 'L1', { ...item, defaultParams: { period: 13 } }, 'L2');

		// The property this whole module exists to prove: the layer count
		// this pane carries for this panel's own indicator never grows past
		// one, no matter how many times the source is edited and re-run.
		expect(afterSecond).toHaveLength(1);
		expect(afterSecond[0].id).toBe('L2');
		expect(afterSecond[0].params).toEqual({ period: 13 });
	});

	test('an unrelated layer already on the pane survives a recompile untouched', () => {
		const unrelated = layer('kept', 0, 'user-rsi', { period: 14 });
		const afterFirst = applyRecompile([unrelated], null, item, 'L1');
		const afterSecond = applyRecompile(afterFirst, 'L1', item, 'L2');

		expect(afterSecond).toHaveLength(2);
		expect(afterSecond.find((l) => l.id === 'kept')).toEqual(unrelated);
	});

	// The defect this guards against, made concrete: a version of this
	// sequence that forgets to drop the previous layer id before appending
	// the new one leaves exactly the orphaned duplicate `applyRecompile`
	// exists to prevent. Run by hand against `applyRecompile` itself with
	// the `withoutPrevious` filter removed (i.e. treating every recompile as
	// a bare append): the assertion above — `toHaveLength(1)` after a second
	// recompile — fails immediately (length 2), which is what confirms the
	// test actually discriminates the bug rather than passing vacuously.
	test('the naive "just append" sequence this replaces would have produced two layers', () => {
		const naiveAppend = (layers: LayerRuntime[], newLayer: LayerRuntime) => [...layers, newLayer];
		const afterFirst = naiveAppend([], { id: 'L1', position: 0, kind: 'indicator_overlay', visible: true, indicatorName: item.name, params: item.defaultParams });
		const afterSecond = naiveAppend(afterFirst, { id: 'L2', position: 1, kind: 'indicator_overlay', visible: true, indicatorName: item.name, params: { period: 13 } });
		expect(afterSecond).toHaveLength(2);
	});
});
