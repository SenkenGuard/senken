import { describe, expect, test } from 'bun:test';
import {
	HISTORY_PAGE_BARS,
	MAX_HISTORY_BARS,
	historyLoadPriority,
	historyWindowAround,
	previousHistoryPage,
	retainedHistoryStart,
	shouldPrefetchHistory
} from './chart-window';

describe('chart history window', () => {
	test('left-edge paging requests the contiguous predecessor page', () => {
		const current = { from: 300, to: 600 };
		expect(previousHistoryPage(current)).toEqual({ from: 0, to: 300 });
		expect(shouldPrefetchHistory(80, HISTORY_PAGE_BARS)).toBe(true);
		expect(shouldPrefetchHistory(81, HISTORY_PAGE_BARS)).toBe(false);
		expect(historyLoadPriority()).toBe('prefetch');
	});

	test('a date jump replaces the old window instead of making two islands', () => {
		const jumped = historyWindowAround(10_000, 10);
		expect(jumped).toEqual({ from: 8_500, to: 11_500 });
		expect(jumped.to - jumped.from).toBe(HISTORY_PAGE_BARS * 10);
	});

	test('retention uses a separate high watermark from the prefetch threshold', () => {
		expect(retainedHistoryStart(MAX_HISTORY_BARS)).toBe(0);
		expect(retainedHistoryStart(MAX_HISTORY_BARS + HISTORY_PAGE_BARS)).toBe(HISTORY_PAGE_BARS);
	});
});
