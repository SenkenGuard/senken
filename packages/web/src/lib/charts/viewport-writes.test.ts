// A structural guard, not a unit test of behaviour.
//
// Four rounds of "the chart jumped" all came back to the same shape: a
// `setData` that replaces a series without holding the viewport across it.
// Replacing data is a structural update — the chart may re-anchor, and if the
// new series starts earlier than the old one every logical index is renumbered
// — while the reader asked for none of it. Two of the four call sites were
// unprotected, and one carried a comment arguing that leaving it unprotected
// was what kept the viewport still.
//
// Reading the source is the only way to state "every one of them is held":
// the rule is about the call sites themselves, not about a value any function
// returns.
import { describe, expect, test } from 'bun:test';
import { readFileSync } from 'node:fs';

const PANE = 'src/lib/components/terminal/chart-pane.svelte';

function sourceLines(): string[] {
	return readFileSync(PANE, 'utf8').split('\n');
}

/** Whether a series write sits inside a `holdViewport` callback, judged by
 * the nearest preceding non-comment line that opens one.
 *
 * `holdViewport` is the pane's single entry point for this: it supplies the
 * pending frame, which is what stops a series update from reading a range the
 * chart has not applied yet and writing the old one back over it. Calling the
 * underlying helper directly would skip that, so the name checked here is
 * deliberately the wrapper's. */
function isHeld(lines: string[], index: number): boolean {
	for (let i = index; i >= 0 && i > index - 12; i -= 1) {
		const line = lines[i].trim();
		if (line.startsWith('//') || line.startsWith('*')) continue;
		if (line.includes('holdViewport(')) return true;
	}
	return false;
}

/** The one series allowed to move the reader: a new candle should carry the
 * view forward, which is what `shiftVisibleRangeOnNewBar` is for. Every
 * derived series must not — an indicator's own first reading for the forming
 * bar is a point that series does not have yet, and each one shifted the
 * chart another bar to the right. */
const CANDLE_UPDATE = 'series.update(';

describe('every series write holds the reader viewport', () => {
	test('no series data is replaced unheld', () => {
		const lines = sourceLines();
		const unheld = lines
			.map((line, index) => ({ line, index }))
			.filter(({ line }) => line.includes('.setData('))
			.filter(({ index }) => !isHeld(lines, index))
			.map(({ index }) => index + 1);

		expect(unheld).toEqual([]);
	});

	test('no series but the candles may shift the view by growing', () => {
		const lines = sourceLines();
		const unheld = lines
			.map((line, index) => ({ line, index }))
			.filter(({ line }) => line.includes('.update(') && !line.includes(CANDLE_UPDATE))
			.filter(({ index }) => !isHeld(lines, index))
			.map(({ index }) => index + 1);

		expect(unheld).toEqual([]);
	});

	test('the candle update is deliberately left free to carry the reader forward', () => {
		const lines = sourceLines();
		const candle = lines.findIndex((line) => line.includes(CANDLE_UPDATE));
		expect(candle).toBeGreaterThan(-1);
		expect(isHeld(lines, candle)).toBe(false);
	});

	test('there are writes to guard at all, so none of this can pass vacuously', () => {
		const lines = sourceLines();
		expect(lines.filter((l) => l.includes('.setData(')).length).toBeGreaterThanOrEqual(4);
		expect(lines.filter((l) => l.includes('.update(')).length).toBeGreaterThanOrEqual(2);
	});
});
