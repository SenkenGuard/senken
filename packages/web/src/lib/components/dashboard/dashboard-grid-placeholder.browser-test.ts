// Proves the property `030`'s own "no keanehan" bar names for a disabled
// plugin: a widget it contributed becomes a placeholder in place — same
// cell, same size, its stored config untouched — the moment the effective
// catalog says its provider is gone, with no reload; and it comes back the
// same way the moment the catalog says it is back. Real reactivity, not
// description: this mounts the real `DashboardGrid`, calls the real
// `registerPluginWidgets` `rendererFor` reads from, and drives the `catalog`
// prop change the same way `routes/dashboard/+page.svelte` actually would
// after `refreshWidgetCatalog()` runs.
import { test, expect } from 'vitest';
import { render } from 'vitest-browser-svelte';
import DashboardGrid from './dashboard-grid.svelte';
import { registerPluginWidgets } from './widget-registry.svelte';
import type { DashboardLayoutDto, WidgetPluginDefinition } from './api';

const PLUGIN_WIDGET: WidgetPluginDefinition = {
	widget_type_id: 'acme/gauge',
	provider_id: 'acme',
	title: 'Gauge',
	description: 'a third-party widget',
	category: 'utility',
	default_size: { width: 3, height: 3 },
	min_size: { width: 2, height: 2 },
	config_schema_version: 1,
	config_schema: {},
	required_permissions: [],
	required_capabilities: [],
	data_source: 'mock',
	entry_url: 'about:blank'
};

function layoutWithPluginWidget(): DashboardLayoutDto {
	return {
		workspace: { id: 'ws1', owner_id: 'u1', name: 'Default', columns: 6, revision: 1, created_at: 0, updated_at: 0 },
		widgets: [
			{
				id: 'w1',
				provider_id: 'acme',
				widget_type_id: 'acme/gauge',
				position_x: 2,
				position_y: 1,
				width: 3,
				height: 3,
				visible: true,
				config: '{"threshold":42}',
				config_schema_version: 1,
				created_at: 0,
				updated_at: 0
			}
		]
	};
}

function cellStyle(): string {
	const el = document.querySelector('[data-widget-id="w1"]');
	if (!el) throw new Error('widget cell w1 not found');
	return el.getAttribute('style') ?? '';
}

test('disabling a plugin turns its placed widget into a placeholder in place, and re-enabling brings it back', async () => {
	registerPluginWidgets([PLUGIN_WIDGET]);
	const layout = layoutWithPluginWidget();

	const screen = await render(DashboardGrid, { layout, catalog: [PLUGIN_WIDGET] });

	expect(document.querySelector('[data-widget-plugin-iframe]')).not.toBeNull();
	expect(document.querySelector('[data-widget-placeholder]')).toBeNull();
	const styleWhileActive = cellStyle();

	// The same two calls `plugins-section.svelte`'s own `setWidgetEnabled`
	// makes after a successful disable (`widget-catalog.svelte.ts`'s
	// `refreshWidgetCatalog`): the module-level registry `rendererFor`
	// reads, and the `catalog` prop this grid actually re-renders on.
	registerPluginWidgets([]);
	await screen.rerender({ layout, catalog: [] });

	expect(document.querySelector('[data-widget-plugin-iframe]')).toBeNull();
	expect(document.querySelector('[data-widget-placeholder]')).not.toBeNull();
	// Same cell, same size, same position — a placeholder is a property of
	// the cell, never a change to the widget's own stored geometry.
	expect(cellStyle()).toBe(styleWhileActive);

	registerPluginWidgets([PLUGIN_WIDGET]);
	await screen.rerender({ layout, catalog: [PLUGIN_WIDGET] });

	expect(document.querySelector('[data-widget-plugin-iframe]')).not.toBeNull();
	expect(document.querySelector('[data-widget-placeholder]')).toBeNull();
	expect(cellStyle()).toBe(styleWhileActive);
});
