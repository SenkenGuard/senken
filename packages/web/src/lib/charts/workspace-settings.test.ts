import { describe, expect, test } from 'bun:test';
import {
	defaultWorkspaceSettings,
	parseWorkspaceSettings,
	resolveShown,
	workspaceSettingsToJson
} from './workspace-settings';

describe('defaultWorkspaceSettings — an unconfigured workspace starts blank', () => {
	test('the default shows no watchlist groups and no notes', () => {
		expect(defaultWorkspaceSettings()).toEqual({ watchlistGroups: [], notes: [] });
	});
});

describe("parseWorkspaceSettings — a workspace's settings text must never break its panels", () => {
	test('the documented empty case, "{}", resolves to the blank default', () => {
		expect(parseWorkspaceSettings('{}')).toEqual(defaultWorkspaceSettings());
	});

	test('a partial object merges over the defaults rather than replacing them', () => {
		const result = parseWorkspaceSettings('{"watchlistGroups":["g1","g2"]}');
		expect(result.watchlistGroups).toEqual(['g1', 'g2']);
		// Not mentioned — keeps its default, proving this is a merge, not a
		// full replace that would leave the other field undefined.
		expect(result.notes).toEqual([]);
	});

	test('malformed JSON falls back to defaults instead of throwing', () => {
		expect(() => parseWorkspaceSettings('{not valid json')).not.toThrow();
		expect(parseWorkspaceSettings('{not valid json')).toEqual(defaultWorkspaceSettings());
	});

	test('valid JSON that is not an object (an array, a bare number) also falls back to defaults', () => {
		expect(parseWorkspaceSettings('[1,2,3]')).toEqual(defaultWorkspaceSettings());
		expect(parseWorkspaceSettings('42')).toEqual(defaultWorkspaceSettings());
		expect(parseWorkspaceSettings('null')).toEqual(defaultWorkspaceSettings());
	});

	test('an empty string (never valid JSON) falls back to defaults too', () => {
		expect(parseWorkspaceSettings('')).toEqual(defaultWorkspaceSettings());
	});

	test('a key this build does not know is dropped rather than carried back out', () => {
		// Text written by a future or experimental build might name a field
		// this build never added a control for; a blanket merge would keep
		// re-saving it forever even though nothing here ever reads it.
		const result = parseWorkspaceSettings('{"watchlistGroups":["g1"],"pinnedAlerts":["a1"]}');
		expect(Object.keys(result)).not.toContain('pinnedAlerts');
		expect(result.watchlistGroups).toEqual(['g1']);
	});

	test('a key holding the wrong type is refused, not handed to the panels', () => {
		const result = parseWorkspaceSettings('{"watchlistGroups":"g1","notes":["n1"]}');
		expect(result.watchlistGroups).toEqual(defaultWorkspaceSettings().watchlistGroups);
		// The well-typed neighbour in the same object still applies, so this
		// rejects a field rather than the whole record.
		expect(result.notes).toEqual(['n1']);
	});

	test('an array with non-string entries falls back to the default for that field', () => {
		expect(parseWorkspaceSettings('{"watchlistGroups":[1,2,3]}').watchlistGroups).toEqual(
			defaultWorkspaceSettings().watchlistGroups
		);
		expect(parseWorkspaceSettings('{"notes":["n1",null]}').notes).toEqual(
			defaultWorkspaceSettings().notes
		);
	});
});

describe('workspaceSettingsToJson round-trip', () => {
	test('a changed settings object survives being serialised and re-parsed', () => {
		const changed = { watchlistGroups: ['g1', 'g2'], notes: ['n1'] };
		const roundTripped = parseWorkspaceSettings(workspaceSettingsToJson(changed));
		expect(roundTripped).toEqual(changed);
	});
});

describe('resolveShown — which owned rows are shown in a workspace', () => {
	const groups = [
		{ id: 'g1', name: 'Majors' },
		{ id: 'g2', name: 'Alts' },
		{ id: 'g3', name: 'Watching' }
	];

	test('shows nothing when the workspace has nothing shown', () => {
		expect(resolveShown(groups, [])).toEqual([]);
	});

	test('returns owned rows in the owning list order, not the stored order', () => {
		// Stored ids name g3 before g1; the owning list's own order (g1, g2,
		// g3) is what the panel renders in, so g1 must still come first.
		expect(resolveShown(groups, ['g3', 'g1'])).toEqual([groups[0], groups[2]]);
	});

	test('ignores ids the user no longer owns, rather than rendering a blank row', () => {
		// The real case this guards: a note (or group) deleted in one
		// workspace leaves its id behind in another workspace's settings.
		expect(resolveShown(groups, ['g1', 'gone', 'g2'])).toEqual([groups[0], groups[1]]);
	});
});
