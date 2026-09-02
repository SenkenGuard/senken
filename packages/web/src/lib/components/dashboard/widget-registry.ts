// The client half of "one manifest, two consumers": the server's own
// `senken_dashboard::WidgetRegistry` (served at `GET
// /api/dashboard/widgets/catalog`) is the source of truth for a built-in
// widget type's title, description, size and `dataSource` — nothing here
// duplicates that metadata. What only the client can know is which Svelte
// component actually draws a given `widget_type_id`, so that binding lives
// here, keyed by the exact same id the server uses.
//
// This replaces the old conditional render path
// (`{#if w.type === 'equity'} ... {:else if w.type === 'watchlist'} ...`)
// with a lookup by id: `dashboard-grid.svelte` never branches on a widget's
// type by name, it asks this module for a renderer and falls back to a
// placeholder when none is registered — the same fallback a disabled or
// not-yet-installed plugin's widget produces.
//
// A dynamic widget UI package's own metadata (title, description, size,
// `dataSource`, and the URL serving its bundle) comes from
// `GET /api/widget-plugins/catalog` the same way — this module never
// invents a renderer for a `widget_type_id` no active catalog names.
// What is different for a plugin widget is *which* component draws it:
// every one of them shares the single generic `plugin-widget-frame.svelte`
// sandboxed-iframe host, parameterised per instance by
// `registerPluginWidgets`/`rendererFor` below, rather than each plugin
// getting its own bespoke Svelte component the way a built-in does.

import type { Component } from 'svelte';

import type { DashboardWidgetDto, WidgetPluginDefinition } from './api';
import PluginWidgetFrame from './plugin-widget-frame.svelte';
import EquityCard from '$lib/components/terminal/equity-card.svelte';
import PositionsPanel from '$lib/components/terminal/positions-panel.svelte';
import RiskPanel from '$lib/components/terminal/risk-panel.svelte';
import PositionSizeCard from './position-size-card.svelte';
// Equity, positions and risk are not fixtures: they are derived from the
// accounts and portfolios the trade engine actually holds, through the same
// three functions the trade dashboard itself renders from. The catalog used
// to ship five more built-ins (watchlist, volatility heatmap, signal desk,
// buy/sell flow, news tape) that rendered invented numbers next to these —
// removed rather than merely labelled mock, since a widget that looks real
// beside one that reads real prices is worse than no widget at all.
//
// The position-size calculator beside them needs no market or account data
// either, but for the opposite reason those five were removed: nothing
// about its answer is invented, so it is not a mockup and does not belong
// behind a plugin's sandboxed iframe just to avoid reading `tradeStore` —
// it is a built-in exactly like the three above, only one that computes
// its answer from what the user types instead of from an adapter. A
// third-party plugin author still gets the equivalent shape as an example
// to build from — see the built-in widget-plugin package this server
// installs on every fresh start.
import { tradeStore } from '$lib/state/trade.svelte';
import { dashboardEquity, dashboardPositions, dashboardRisk } from '$lib/trade/view';

/** One registered widget type's render binding: the component that draws
 * it, and a function producing the props it is mounted with.
 *
 * A function rather than a fixed object, because three of these widgets now
 * read live account and portfolio state: evaluating their props once, when
 * this module is first imported, would freeze whatever the trade engine
 * happened to hold at that moment and never update again. */
export interface ClientWidgetRenderer {
	// Each entry's component has its own distinct props type; the map
	// itself is necessarily heterogeneous, the same way the server's own
	// catalog is untyped past `widget_type_id`.
	component: Component<any>;
	// `object`, not `Record<string, unknown>`: each widget's props type is a
	// concrete interface with named fields and no index signature (e.g.
	// `DashboardEquityData`), and TypeScript does not consider a plain
	// interface assignable to an indexed type — only `object` is broad
	// enough to admit every one of them while still ruling out a bare
	// primitive.
	props: () => object;
}

/** The render binding for every built-in widget this build ships,
 * keyed by the exact `widget_type_id` `senken_dashboard::WidgetRegistry::builtin`
 * assigns each one (`<provider_id>/<widget>`, provider `"senken"`). */
const BUILTIN_RENDERERS: Record<string, ClientWidgetRenderer> = {
	'senken/equity': {
		component: EquityCard as Component<any>,
		props: () => ({ data: dashboardEquity(tradeStore.ownAccounts, tradeStore.portfolios) })
	},
	'senken/positions': {
		component: PositionsPanel as Component<any>,
		props: () => ({ table: dashboardPositions(tradeStore.ownAccounts, tradeStore.portfolios) })
	},
	'senken/risk': {
		component: RiskPanel as Component<any>,
		props: () => ({ data: dashboardRisk(tradeStore.ownAccounts, tradeStore.portfolios) })
	}
};

/** `senken/position-size`'s own `widget_type_id` — handled outside
 * [`BUILTIN_RENDERERS`] because, unlike the three in that map, it needs
 * *this placed instance's* own `config` and `onConfigChange`, not shared
 * trade-engine state every instance reads alike. */
const POSITION_SIZE_WIDGET_TYPE_ID = 'senken/position-size';

/** Every widget a currently active widget plugin package contributes,
 * keyed by `widget_type_id` — populated by `registerPluginWidgets` once
 * `GET /api/widget-plugins/catalog` resolves, and read by `rendererFor`
 * below. A `Map`, not a `$state`: nothing here needs to be reactive on its
 * own, since the caller re-renders `dashboard-grid.svelte` by keying it on
 * the workspace id, and a catalog refresh reloads the whole page's data
 * the same way the built-in catalog already does. */
let pluginWidgets = new Map<string, WidgetPluginDefinition>();

/** Replaces the plugin-widget catalog `rendererFor` resolves dynamic
 * widgets against. Called once the effective catalog
 * (`GET /api/dashboard/widgets/catalog` merged with
 * `GET /api/widget-plugins/catalog`) is fetched — see
 * `routes/dashboard/+page.svelte`. Disabling a plugin's package removes its
 * entries from the *next* catalog fetch, which is what makes a placed
 * instance of one of its widgets fall back to `rendererFor` returning
 * `undefined` (a placeholder) the moment that reload lands — the exact
 * same fallback path an uninstalled built-in already uses. */
export function registerPluginWidgets(definitions: WidgetPluginDefinition[]): void {
	pluginWidgets = new Map(definitions.map((definition) => [definition.widget_type_id, definition]));
}

/** Looks up the render binding for `widget`, or `undefined` when this
 * build has no renderer for its type — a provider disabled, failed to
 * load, or simply not installed. The caller (`dashboard-grid.svelte`)
 * treats `undefined` as "render a placeholder in this widget's cell,
 * unchanged size, config untouched", never as an error.
 *
 * Most built-ins' props are the same shared fixture-free trade-engine
 * state every call reads alike; `senken/position-size` and every plugin
 * widget instead build their props fresh per call from `widget`'s own id
 * and config, since each placed instance of one of those can hold
 * different config the widget itself reads back and patches. */
export function rendererFor(
	widget: DashboardWidgetDto,
	onConfigChange?: (config: string) => void
): ClientWidgetRenderer | undefined {
	if (widget.widget_type_id === POSITION_SIZE_WIDGET_TYPE_ID) {
		return {
			component: PositionSizeCard as Component<any>,
			props: () => ({ config: widget.config, onConfigChange })
		};
	}

	const builtin = BUILTIN_RENDERERS[widget.widget_type_id];
	if (builtin) return builtin;

	const plugin = pluginWidgets.get(widget.widget_type_id);
	if (!plugin) return undefined;
	return {
		component: PluginWidgetFrame as Component<any>,
		props: () => ({
			entryUrl: plugin.entry_url,
			widgetTypeId: widget.widget_type_id,
			config: widget.config,
			onConfigChange
		})
	};
}
