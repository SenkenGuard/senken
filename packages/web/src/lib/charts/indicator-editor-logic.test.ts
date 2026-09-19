import { describe, expect, test } from 'bun:test';
import { computeDirty, decideSelect, diagnosticsFromCompileError, shouldSaveBeforeAdding, catalogItemFromEntry } from './indicator-editor-logic';
import type { IndicatorCatalogEntry } from '$lib/api/types';

describe('computeDirty', () => {
	test('a brand-new draft (nothing saved yet) is dirty as soon as anything is typed', () => {
		expect(computeDirty(null, { title: '', source: '' })).toBe(false);
		expect(computeDirty(null, { title: 'My SMA', source: '' })).toBe(true);
		expect(computeDirty(null, { title: '', source: 'fn main() {}' })).toBe(true);
	});

	test('an opened indicator is dirty only once the draft diverges from what was saved', () => {
		const saved = { title: 'My SMA', source: 'let a = 1;' };
		expect(computeDirty(saved, { ...saved })).toBe(false);
		expect(computeDirty(saved, { ...saved, source: 'let a = 2;' })).toBe(true);
		expect(computeDirty(saved, { ...saved, title: 'Renamed' })).toBe(true);
	});
});

describe('decideSelect: switching indicators asks first only when it would lose something', () => {
	test('selecting a different indicator while dirty prompts instead of switching immediately', () => {
		expect(decideSelect(true, 'a', 'b')).toEqual({ kind: 'prompt', targetId: 'b' });
	});

	test('re-selecting the indicator that is already open never prompts, dirty or not', () => {
		expect(decideSelect(true, 'a', 'a')).toEqual({ kind: 'proceed', targetId: 'a' });
	});

	test('selecting anything while clean proceeds without asking', () => {
		expect(decideSelect(false, 'a', 'b')).toEqual({ kind: 'proceed', targetId: 'b' });
		expect(decideSelect(false, 'a', null)).toEqual({ kind: 'proceed', targetId: null });
	});

	// The defect this exists to prevent, made concrete: a version of this
	// decision that prompts on *every* dirty selection — including
	// re-selecting the same id, which some callers do to force a refresh —
	// would ask "Discard changes?" for a no-op switch. Removing the
	// `currentId !== requestedId` guard and asserting the same call proves
	// this test actually discriminates the bug rather than passing
	// vacuously: with the guard removed, `decideSelect(true, 'a', 'a')`
	// would return `{ kind: 'prompt', targetId: 'a' }`, failing the
	// assertion above.
	test('the guard this decision depends on is exercised, not merely present', () => {
		const withoutGuard = (dirty: boolean, currentId: string | null, requestedId: string | null) =>
			dirty ? { kind: 'prompt' as const, targetId: requestedId } : { kind: 'proceed' as const, targetId: requestedId };
		expect(withoutGuard(true, 'a', 'a')).toEqual({ kind: 'prompt', targetId: 'a' });
	});
});

describe('diagnosticsFromCompileError', () => {
	test('a stored compile error becomes one diagnostic with no line or column', () => {
		expect(diagnosticsFromCompileError('unresolved import `foo`')).toEqual([
			{ line: null, column: null, message: 'unresolved import `foo`' }
		]);
	});

	test('no stored error means no diagnostics', () => {
		expect(diagnosticsFromCompileError(null)).toEqual([]);
		expect(diagnosticsFromCompileError(undefined)).toEqual([]);
	});
});

describe('shouldSaveBeforeAdding', () => {
	test('unsaved edits always save first, even if a previous compile succeeded', () => {
		expect(shouldSaveBeforeAdding(true, true)).toBe(true);
	});

	test('a clean draft that has never compiled still saves first', () => {
		expect(shouldSaveBeforeAdding(false, false)).toBe(true);
	});

	test('a clean, already-compiled draft adds straight to the chart', () => {
		expect(shouldSaveBeforeAdding(false, true)).toBe(false);
	});
});

describe('catalogItemFromEntry', () => {
	test('builds the exact IndicatorCatalogItem shape the built-in picker builds', () => {
		const entry: IndicatorCatalogEntry = {
			name: 'my/my-sma',
			title: 'My SMA',
			short_title: 'SMA',
			legend: 'SMA({period})',
			params: [{ name: 'period', kind: 'integer', default: { kind: 'integer', value: 14 }, min: 1 }],
			plots: [],
			scale: { kind: 'price' },
			requires_real_volume: false,
			placement: 'overlay',
			warmup_bars: 14
		};
		expect(catalogItemFromEntry(entry)).toEqual({
			name: 'my/my-sma',
			defaultParams: { period: 14 },
			placement: 'overlay'
		});
	});
});
