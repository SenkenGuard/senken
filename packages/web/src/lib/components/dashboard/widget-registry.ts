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
import FlowPanel from '$lib/components/terminal/flow-panel.svelte';
import HeatPanel from '$lib/components/terminal/heat-panel.svelte';
import NewsFeed from '$lib/components/terminal/news-feed.svelte';
import PositionsPanel from '$lib/components/terminal/positions-panel.svelte';
import RiskPanel from '$lib/components/terminal/risk-panel.svelte';
import SignalFeed from '$lib/components/terminal/signal-feed.svelte';
import WatchlistPanel from '$lib/components/terminal/watchlist-panel.svelte';
import {
	EQUITY,
	FEED,
	FLOW_BARS,
	HEAT_CELLS,
	POSITIONS,
	RISK_BARS,
	SIGNALS,
	WATCHLIST
} from '$lib/mock/dashboard';

/** One registered widget type's render binding: the component that draws
 * it, and the (fixed, for now) props it is mounted with. A widget with
 * real per-instance config would read it from a placed widget's own
 * `config` field instead of a fixture import — none of today's built-ins
 * take any config yet, so every entry's props are the same fixture data
 * `routes/+page.svelte`'s own demo already uses. */
export interface ClientWidgetRenderer {
	// Each entry's component has its own distinct props type; the map
	// itself is necessarily heterogeneous, the same way the server's own
	// catalog is untyped past `widget_type_id`.
	component: Component<any>;
	props: Record<string, unknown>;
}

/** The render binding for every built-in widget this build ships,
 * keyed by the exact `widget_type_id` `senken_dashboard::WidgetRegistry::builtin`
 * assigns each one (`<provider_id>/<widget>`, provider `"senken"`). */
const BUILTIN_RENDERERS: Record<string, ClientWidgetRenderer> = {
	'senken/equity': { component: EquityCard as Component<any>, props: { equity: EQUITY } },
	'senken/watchlist': {
		component: WatchlistPanel as Component<any>,
		props: { rows: WATCHLIST }
	},
	'senken/positions': {
		component: PositionsPanel as Component<any>,
		props: { rows: POSITIONS }
	},
	'senken/risk': { component: RiskPanel as Component<any>, props: { bars: RISK_BARS } },
	'senken/heatmap': { component: HeatPanel as Component<any>, props: { cells: HEAT_CELLS } },
	'senken/signals': { component: SignalFeed as Component<any>, props: { signals: SIGNALS } },
	'senken/flow': { component: FlowPanel as Component<any>, props: { bars: FLOW_BARS } },
	'senken/feed': { component: NewsFeed as Component<any>, props: { items: FEED } }
};

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
 * A built-in widget's props are the same fixed fixture data every call
 * gets (none of today's built-ins read per-instance config yet); a plugin
 * widget's props are built fresh per call from `widget`'s own id and
 * config, since — unlike a built-in — every dynamic widget instance can
 * hold different config the iframe reads back through `config.get`. */
export function rendererFor(
	widget: DashboardWidgetDto,
	onConfigChange?: (config: string) => void
): ClientWidgetRenderer | undefined {
	const builtin = BUILTIN_RENDERERS[widget.widget_type_id];
	if (builtin) return builtin;

	const plugin = pluginWidgets.get(widget.widget_type_id);
	if (!plugin) return undefined;
	return {
		component: PluginWidgetFrame as Component<any>,
		props: {
			entryUrl: plugin.entry_url,
			widgetTypeId: widget.widget_type_id,
			config: widget.config,
			onConfigChange
		}
	};
}
