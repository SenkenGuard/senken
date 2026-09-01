// The storage panel's own pure decisions: how a size reads, what a tree
// node is called, and what a deletion is about to remove.
//
// Kept out of the component for the usual reason this codebase splits such
// things out — a `.svelte` file cannot be exercised by `bun test` without a
// DOM, and these are exactly the rules that must be right: a confirmation
// that understates what it is about to delete is worse than no confirmation
// at all.

/** One byte figure, rendered for a person.
 *
 * Binary units, because that is what a filesystem reports and what the
 * reader will see if they check the directory themselves — showing 1.05 MB
 * where `du` says 1.0 MiB invites them to think something is unaccounted
 * for. One decimal place above a kilobyte and none below it: a file of
 * "812.0 B" is noise, and three decimals on a gigabyte is false precision
 * about a number that changes as the app runs.
 *
 * Exact zero still renders as `0 B` rather than as a dash: zero is a
 * measurement, not a missing one. Whether a node holding zero is *listed*
 * at all is a separate question, answered by `hasStoredData` below. */
export function formatBytes(bytes: number): string {
	if (!Number.isFinite(bytes) || bytes < 0) return '—';
	if (bytes < 1024) return `${Math.round(bytes)} B`;
	const units = ['KB', 'MB', 'GB', 'TB', 'PB'];
	let value = bytes / 1024;
	let unit = 0;
	while (value >= 1024 && unit < units.length - 1) {
		value /= 1024;
		unit += 1;
	}
	return `${value.toFixed(1)} ${units[unit]}`;
}

/** A file count, rendered for a person — `1 file`, `12 files`. Spelled out
 * rather than shown as a bare number so a row reading "3" cannot be
 * mistaken for a size. */
export function formatFiles(files: number): string {
	return `${files} ${files === 1 ? 'file' : 'files'}`;
}

/** Anything the panel measures: a source, an instrument, a series. */
export interface StorageMeasured {
	bytes: number;
	files: number;
}

/** Whether a node is holding anything at all.
 *
 * A server can have dozens of registered sources and have fetched from two
 * of them. Listing the other twenty-odd at `0 B` fills the panel with rows
 * that answer a question nobody asked and bury the two that matter — the
 * panel exists to show where the disk went, and a node using none of it is
 * not part of that answer.
 *
 * Both fields are checked, not just one. A directory holding files that are
 * themselves empty is `0 B` across `n files` and is still a real thing on
 * disk worth offering to remove; a node with neither is nothing. */
export function hasStoredData(node: StorageMeasured): boolean {
	return node.files > 0 || node.bytes > 0;
}

/** The nodes worth listing, at any level of the tree — sources,
 * instruments, or series. Generic so one rule serves all three rather than
 * three filters that could drift apart. */
export function withStoredData<T extends StorageMeasured>(nodes: readonly T[]): T[] {
	return nodes.filter(hasStoredData);
}

/** Which node of the tree a row is. Every level is addressed by the same
 * shape so one expand/collapse map and one delete handler serve all three,
 * rather than three parallel sets of state that can disagree about what is
 * open. */
export interface StorageNodeId {
	sourceId: string;
	symbol?: string;
	seriesId?: string;
}

/** A stable key for `{#each}` and for the expanded-node set.
 *
 * Length-prefixed rather than joined on a separator, because there is no
 * character that cannot appear in one of these parts: a symbol reaches the
 * client decoded, so it may hold a slash, a colon or anything else a venue
 * chose to put in a ticker. Prefixing each part with its own length makes a
 * collision impossible by construction instead of by hoping — and two rows
 * sharing a key is how the wrong row ends up expanded, or deleted.
 *
 * (This used to join on a NUL character, which was both unprovable and
 * written into the file as a literal NUL byte, leaving a TypeScript source
 * file that git treated as binary.) */
export function storageNodeKey(id: StorageNodeId): string {
	return [id.sourceId, id.symbol ?? '', id.seriesId ?? '']
		.map((part) => `${part.length}:${part}`)
		.join('');
}

/** What a node holds, for the confirmation. */
export interface StorageNodeSummary {
	bytes: number;
	files: number;
}

/** The sentence a delete confirmation shows.
 *
 * Names the thing being removed at the level it was asked for, states what
 * it costs to be wrong, and says the one thing that decides whether this is
 * safe: market data is re-fetchable, so the loss is time and venue
 * requests, not the data itself. A confirmation that only says "are you
 * sure?" makes the reader guess at all three.
 */
export function describeStorageDeletion(
	id: StorageNodeId,
	summary: StorageNodeSummary,
	seriesLabel?: string
): string {
	const size = `${formatBytes(summary.bytes)} across ${formatFiles(summary.files)}`;
	const recoverable = 'It can be downloaded again from the venue.';
	if (id.seriesId) {
		const what = seriesLabel ?? id.seriesId;
		return `Delete ${what} for ${id.symbol} on ${id.sourceId}? This frees ${size}. ${recoverable}`;
	}
	if (id.symbol) {
		return `Delete every stored series for ${id.symbol} on ${id.sourceId}? This frees ${size}. ${recoverable}`;
	}
	// Says the reach first and the cost second. Appending "across every
	// instrument" to a size that already reads "4.1 MB across 3 files" gave
	// one sentence two "across"es meaning different things.
	return `Delete everything stored for ${id.sourceId}, for every instrument from that source? This frees ${size}. ${recoverable}`;
}

/** How much of the whole a node accounts for, as a percentage in `[0, 100]`.
 *
 * `0` when the total is zero rather than `NaN` from the division: a fresh
 * install has nothing stored, and a bar rendered from `NaN` silently
 * collapses to nothing while a bar rendered from `0` is correct. */
export function shareOfTotal(bytes: number, totalBytes: number): number {
	if (!Number.isFinite(bytes) || !Number.isFinite(totalBytes) || totalBytes <= 0) return 0;
	return Math.min(100, Math.max(0, (bytes / totalBytes) * 100));
}
