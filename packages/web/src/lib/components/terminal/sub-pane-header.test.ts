// Renders the real, compiled component through Svelte's own server renderer
// and reads the actual output markup, rather than asserting on the logic that
// is supposed to produce it.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import SubPaneHeader from './sub-pane-header.svelte';

const layer = {
	id: 'layer-1',
	kind: 'indicator_sub_pane' as const,
	indicatorName: 'Rsi',
	params: { period: 14 },
	visible: true,
	position: 0,
	style: {}
};

const noop = () => {};
const props = {
	layer,
	onToggleLayer: noop,
	onOpenSettings: noop,
	onRemoveLayer: noop
};

describe('sub-pane header (real DOM output)', () => {
	test('idle shows its actions and no spinner', () => {
		const { body } = render(SubPaneHeader, { props: { ...props, loading: false } });
		expect(body).not.toContain('animate-spin');
		expect(body).toContain('aria-label="Hide Rsi"');
		expect(body).toContain('aria-label="Rsi settings"');
		expect(body).toContain('aria-label="Remove Rsi"');
	});

	// Two controls where there was one shifts the row under the cursor, and an
	// action pressed mid-fetch acts on a series that is about to be replaced.
	// The spinner therefore takes the actions' place rather than joining them.
	test('while loading the spinner replaces the actions rather than joining them', () => {
		const { body } = render(SubPaneHeader, { props: { ...props, loading: true } });
		expect(body).toContain('animate-spin');
		expect(body).toContain('data-loading="layer-1"');
		expect(body).not.toContain('aria-label="Hide Rsi"');
		expect(body).not.toContain('aria-label="Rsi settings"');
		expect(body).not.toContain('aria-label="Remove Rsi"');
	});

	test('the layer still names itself while it works', () => {
		const { body } = render(SubPaneHeader, { props: { ...props, loading: true } });
		expect(body).toContain('Rsi');
		expect(body).toContain('14');
	});
});
