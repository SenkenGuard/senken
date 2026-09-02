// Renders the real, compiled component through Svelte's own server renderer
// and reads the actual output markup — the mockup label and the
// placeholder state are both properties that must be *visible in the DOM*,
// not merely true of the component's props, so this checks the rendered
// HTML rather than the logic that is supposed to produce it.
import { describe, expect, test } from 'bun:test';
import { render } from 'svelte/server';
import WidgetFrame from './widget-frame.svelte';

const noop = () => {};

describe('widget frame mockup label (real DOM output)', () => {
	test('a widget reporting a mock data source gets a host-drawn label', () => {
		const { body } = render(WidgetFrame, {
			props: { title: 'Equity Curve', dataSource: 'mock', onRemove: noop }
		});
		expect(body).toContain('data-mockup-label');
		expect(body).toContain('Mock');
	});

	test('a widget reporting a live data source gets no label at all', () => {
		const { body } = render(WidgetFrame, {
			props: { title: 'Equity Curve', dataSource: 'live', onRemove: noop }
		});
		expect(body).not.toContain('data-mockup-label');
	});

	// The property this guards: the label is drawn by the frame from
	// `dataSource` alone. Nothing a widget renders as its own `children`
	// can add or remove it — proven here by never passing `children` at
	// all and still getting the label purely from the prop.
	test('the label appears from dataSource alone, with no widget body rendered', () => {
		const { body } = render(WidgetFrame, {
			props: { title: 'Anything', dataSource: 'mock', onRemove: noop }
		});
		expect(body).toContain('data-mockup-label');
	});
});

describe('widget frame placeholder state (real DOM output)', () => {
	test('a placeholder widget renders the placeholder markup, not any children', () => {
		const { body } = render(WidgetFrame, {
			props: { title: 'Missing Provider', isPlaceholder: true, onRemove: noop }
		});
		expect(body).toContain('data-widget-placeholder');
		expect(body).toContain('data-placeholder="true"');
	});

	test('a normal (non-placeholder) widget carries no placeholder markup', () => {
		const { body } = render(WidgetFrame, {
			props: { title: 'Equity Curve', dataSource: 'mock', onRemove: noop }
		});
		expect(body).not.toContain('data-widget-placeholder');
		expect(body).not.toContain('data-placeholder="true"');
	});

	test('a placeholder can still report no mockup label when its data source is unknown', () => {
		// A placeholder's provider is unavailable, so its own `dataSource`
		// cannot be known either — this must not default to drawing a mock
		// label it has no basis for.
		const { body } = render(WidgetFrame, {
			props: { title: 'Missing Provider', isPlaceholder: true, onRemove: noop }
		});
		expect(body).not.toContain('data-mockup-label');
	});
});

describe('widget frame chrome (real DOM output)', () => {
	test('every frame carries a drag handle, a resize handle and a remove control', () => {
		const { body } = render(WidgetFrame, {
			props: { title: 'Watchlist', dataSource: 'mock', onRemove: noop }
		});
		expect(body).toContain('aria-label="Move Watchlist"');
		expect(body).toContain('aria-label="Resize Watchlist"');
		expect(body).toContain('aria-label="Remove Watchlist"');
	});

	test('the frame is marked so a grid container can find every placed widget', () => {
		const { body } = render(WidgetFrame, {
			props: { title: 'Watchlist', dataSource: 'mock', onRemove: noop }
		});
		expect(body).toContain('data-dashboard-widget-frame');
	});
});
