import { describe, expect, test } from 'bun:test';
import { syncTargets } from './chart-sync';

describe('syncTargets', () => {
	test('a four-pane layout fans out to the other three', () => {
		expect(syncTargets(0, 4)).toEqual([1, 2, 3]);
		expect(syncTargets(2, 4)).toEqual([0, 1, 3]);
		expect(syncTargets(3, 4)).toEqual([0, 1, 2]);
	});

	test('a single-pane layout has no targets', () => {
		expect(syncTargets(0, 1)).toEqual([]);
	});

	test('never includes the source index itself', () => {
		for (let count = 1; count <= 4; count++) {
			for (let source = 0; source < count; source++) {
				expect(syncTargets(source, count)).not.toContain(source);
			}
		}
	});
});
