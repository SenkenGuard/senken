// Renders the real, compiled component through Svelte's own server renderer
// (`svelte/server`) and reads the actual output markup — the same
// `data-*` attributes a browser would show — rather than asserting on the
// props that are supposed to produce them. Same technique as
// `history-edge-status.test.ts`/`widget-frame.test.ts`.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import IndicatorPanelEditor from './indicator-panel-editor.svelte';

const noop = () => {};

describe('indicator panel editor (real DOM output)', () => {
	test('one gutter line-number cell per source line', () => {
		const { body } = render(IndicatorPanelEditor, {
			props: { source: 'let a = ema(close, 5)\nplot a\n', onSourceChange: noop }
		});
		const cells = body.match(/data-line-number/g) ?? [];
		// `'let a...\n plot a\n'.split('\n')` has three elements (a trailing
		// empty string after the final newline) — the gutter must show one
		// row per array element, not silently drop the trailing blank line a
		// trader's cursor can still sit on.
		expect(cells).toHaveLength(3);
		expect(body).not.toContain('data-compile-error');
	});

	test('a compile error marks its own line in the gutter and states line/column/message in a banner, without touching any other line', () => {
		const { body } = render(IndicatorPanelEditor, {
			props: {
				source: 'let a = ema(close, 5)\nlet b = bogus(a)\nplot b\n',
				error: { line: 2, column: 9, message: 'unknown function `bogus`' },
				onSourceChange: noop
			}
		});

		expect(body).toContain('data-line="1" data-line-error="false"');
		expect(body).toContain('data-line="2" data-line-error="true"');
		expect(body).toContain('data-line="3" data-line-error="false"');
		expect(body).toContain('data-line="4" data-line-error="false"');

		expect(body).toContain('data-compile-error');
		expect(body).toContain('data-line="2"');
		expect(body).toContain('data-column="9"');
		expect(body.replace(/<[^>]+>/g, ' ')).toContain('unknown function `bogus`');
	});

	test('the textarea carries the exact source text, unmodified', () => {
		const source = 'let period = 14\nplot rsi(close, period)\n';
		const { body } = render(IndicatorPanelEditor, { props: { source, onSourceChange: noop } });
		expect(body).toContain('data-indicator-editor-input');
		expect(body).toContain('>let period = 14\nplot rsi(close, period)\n</textarea>');
	});
});
