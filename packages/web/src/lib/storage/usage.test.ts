import { describe, expect, test } from 'bun:test';
import {
	describeStorageDeletion,
	formatBytes,
	formatFiles,
	hasStoredData,
	shareOfTotal,
	storageNodeKey,
	withStoredData
} from './usage';

describe('rendering a size the way the filesystem reports it', () => {
	test('bytes below a kilobyte are whole, not fractional', () => {
		expect(formatBytes(0)).toBe('0 B');
		expect(formatBytes(1)).toBe('1 B');
		expect(formatBytes(812)).toBe('812 B');
		expect(formatBytes(1023)).toBe('1023 B');
	});

	test('units are binary, matching what du and the disk itself say', () => {
		// 1_048_576 bytes is 1.0 MB here and 1.05 MB in decimal units — the
		// discrepancy is exactly what makes a reader think bytes are missing.
		expect(formatBytes(1024)).toBe('1.0 KB');
		expect(formatBytes(1_048_576)).toBe('1.0 MB');
		expect(formatBytes(1_073_741_824)).toBe('1.0 GB');
	});

	test('the unit climbs no further than it has data for', () => {
		expect(formatBytes(1024 ** 5)).toBe('1.0 PB');
		// Beyond the last named unit it stays in it rather than producing a
		// unit nobody defined.
		expect(formatBytes(1024 ** 6)).toBe('1024.0 PB');
	});

	test('a nonsensical figure reads as unknown instead of as zero', () => {
		// Zero is a real state — an empty directory. Unknown must not
		// impersonate it.
		expect(formatBytes(Number.NaN)).toBe('—');
		expect(formatBytes(-1)).toBe('—');
		expect(formatBytes(Number.POSITIVE_INFINITY)).toBe('—');
	});

	test('a file count is spelled out so a row of digits is unambiguous', () => {
		expect(formatFiles(0)).toBe('0 files');
		expect(formatFiles(1)).toBe('1 file');
		expect(formatFiles(12)).toBe('12 files');
	});
});

describe('addressing a node in the tree', () => {
	test('each level has its own key', () => {
		expect(storageNodeKey({ sourceId: 'okx' })).not.toBe(
			storageNodeKey({ sourceId: 'okx', symbol: 'BTCUSDT' })
		);
		expect(storageNodeKey({ sourceId: 'okx', symbol: 'BTCUSDT' })).not.toBe(
			storageNodeKey({ sourceId: 'okx', symbol: 'BTCUSDT', seriesId: 'venue-1m' })
		);
	});

	test('two different nodes cannot collide on one key', () => {
		// The failure this guards: a separator that can occur inside a symbol
		// lets one row's key equal another's, and the wrong row gets expanded
		// — or deleted.
		const a = storageNodeKey({ sourceId: 'okx', symbol: 'BTC/USDT' });
		const b = storageNodeKey({ sourceId: 'okx', symbol: 'BTC', seriesId: 'USDT' });
		expect(a).not.toBe(b);
	});

	test('the same node always produces the same key', () => {
		expect(storageNodeKey({ sourceId: 'okx', symbol: 'ETHUSDT' })).toBe(
			storageNodeKey({ sourceId: 'okx', symbol: 'ETHUSDT' })
		);
	});
});

describe('what a delete confirmation tells the reader', () => {
	const summary = { bytes: 2 * 1024 * 1024, files: 4 };

	test('a series names the series, its instrument and its source', () => {
		const message = describeStorageDeletion(
			{ sourceId: 'okx-spot', symbol: 'BTCUSDT', seriesId: 'venue-1m' },
			summary,
			'1m · venue'
		);
		expect(message).toContain('1m · venue');
		expect(message).toContain('BTCUSDT');
		expect(message).toContain('okx-spot');
	});

	test('every level states the size it frees, and that the data comes back', () => {
		// The two facts that decide whether this is safe to press. A
		// confirmation missing either makes the reader guess.
		for (const id of [
			{ sourceId: 'okx-spot' },
			{ sourceId: 'okx-spot', symbol: 'BTCUSDT' },
			{ sourceId: 'okx-spot', symbol: 'BTCUSDT', seriesId: 'venue-1m' }
		]) {
			const message = describeStorageDeletion(id, summary);
			expect(message).toContain('2.0 MB');
			expect(message).toContain('4 files');
			expect(message).toContain('downloaded again');
		}
	});

	test('deleting a source says it reaches every instrument under it', () => {
		const message = describeStorageDeletion({ sourceId: 'okx-spot' }, summary);
		expect(message).toContain('every instrument');
	});

	test('no sentence uses one word twice for two different things', () => {
		// "frees 4.1 MB across 3 files across every instrument" — the size
		// already spends "across", so the reach has to be worded another way.
		for (const id of [
			{ sourceId: 'okx-spot' },
			{ sourceId: 'okx-spot', symbol: 'BTCUSDT' },
			{ sourceId: 'okx-spot', symbol: 'BTCUSDT', seriesId: 'venue-1m' }
		]) {
			const message = describeStorageDeletion(id, summary);
			expect(message.split('across').length - 1).toBe(1);
		}
	});

	test('a series with no readable label falls back to its directory name', () => {
		// An unrecognised directory still has to be nameable, or it is
		// something on disk the panel cannot offer to remove.
		const message = describeStorageDeletion(
			{ sourceId: 'okx-spot', symbol: 'BTCUSDT', seriesId: 'something-odd' },
			summary
		);
		expect(message).toContain('something-odd');
	});
});

describe('how much of the whole a node accounts for', () => {
	test('a share is a percentage of the total', () => {
		expect(shareOfTotal(50, 200)).toBe(25);
		expect(shareOfTotal(200, 200)).toBe(100);
	});

	test('an empty install reports zero rather than a broken bar', () => {
		// `0/0` is NaN, and a bar width of NaN silently renders as nothing —
		// indistinguishable from a bar that is genuinely empty.
		expect(shareOfTotal(0, 0)).toBe(0);
		expect(shareOfTotal(10, 0)).toBe(0);
	});

	test('a share can never exceed the bar it is drawn in', () => {
		expect(shareOfTotal(300, 200)).toBe(100);
		expect(shareOfTotal(-5, 200)).toBe(0);
	});
});

describe('which nodes are worth listing', () => {
	test('a node holding nothing is not listed', () => {
		// A server registers dozens of sources and fetches from two. The rest
		// would otherwise fill the panel with `0 B` rows that bury the answer
		// it exists to give.
		expect(hasStoredData({ bytes: 0, files: 0 })).toBe(false);
	});

	test('a node holding anything is listed, by either measure', () => {
		expect(hasStoredData({ bytes: 1, files: 0 })).toBe(true);
		// Files that are themselves empty are still real files on disk, and
		// still worth offering to remove.
		expect(hasStoredData({ bytes: 0, files: 1 })).toBe(true);
	});

	test('the same rule serves every level of the tree', () => {
		const nodes = [
			{ source_id: 'okx-spot', bytes: 4_000_000, files: 3 },
			{ source_id: 'never-fetched', bytes: 0, files: 0 },
			{ source_id: 'bybit-spot', bytes: 120_000, files: 1 }
		];
		expect(withStoredData(nodes).map((n) => n.source_id)).toEqual(['okx-spot', 'bybit-spot']);
	});

	test('filtering keeps the order it was given', () => {
		// The server sorts biggest-first; a filter that reordered would put
		// the largest thing somewhere other than the top.
		const nodes = [
			{ id: 'a', bytes: 9, files: 1 },
			{ id: 'b', bytes: 0, files: 0 },
			{ id: 'c', bytes: 5, files: 1 },
			{ id: 'd', bytes: 1, files: 1 }
		];
		expect(withStoredData(nodes).map((n) => n.id)).toEqual(['a', 'c', 'd']);
	});

	test('a tree with nothing in it filters down to nothing', () => {
		expect(withStoredData([{ bytes: 0, files: 0 }])).toEqual([]);
		expect(withStoredData([])).toEqual([]);
	});
});
