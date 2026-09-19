// The dashboard's effective widget catalog — built-ins plus every widget an
// active widget plugin package contributes — held in one module-level
// `$state` instead of a route's own local variable, so a place *outside*
// the dashboard route (Settings → Plugins, enabling/disabling/removing a
// package) can trigger a refresh the dashboard page picks up immediately,
// with no reload and no direct import cycle between the two.
//
// `routes/dashboard/+page.svelte` reads `widgetCatalog.definitions`
// (`$derived`-friendly: a plain reactive read, not a function call) and
// calls `refreshWidgetCatalog()` once on mount; `plugins-section.svelte`
// calls `refreshWidgetCatalog()` again after any action that can change
// which packages are active. `dashboard-grid.svelte`'s own `catalogById` is
// already `$derived` from whatever catalog array it is handed, so a widget
// already placed on the grid re-resolves to a placeholder (or back) the
// moment this next updates — nothing about a placed widget's own stored
// config or geometry is touched by a catalog refresh.
import {
	dashboardWidgetCatalog,
	widgetPluginCatalog,
	type DashboardWidgetDefinition
} from './api';
import { registerPluginWidgets } from './widget-registry.svelte';

class WidgetCatalog {
	definitions = $state<DashboardWidgetDefinition[]>([]);
}

export const widgetCatalog = new WidgetCatalog();

/** Fetches both halves of the effective catalog and registers the plugin
 * half's renderers (`registerPluginWidgets`) — the plugin half degrades to
 * an empty list on failure rather than failing the whole refresh: a
 * widget-plugin package can be unreachable without that being allowed to
 * make the built-in catalog unreachable too. */
export async function refreshWidgetCatalog(): Promise<void> {
	const [builtin, plugins] = await Promise.all([
		dashboardWidgetCatalog(),
		widgetPluginCatalog().catch(() => ({ widgets: [] }))
	]);
	registerPluginWidgets(plugins.widgets);
	widgetCatalog.definitions = [...builtin.widgets, ...plugins.widgets];
}
