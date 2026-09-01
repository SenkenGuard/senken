// Per-layer plot style: colour, width, line style and per-plot visibility.
//
// Held in a session map keyed by layer id so every call site can ask for a
// layer's plots without threading the styling through the component tree,
// and mirrored to the layer's own persisted `style` JSON so the choice
// survives a reload. `hydrateLayerStyle` fills the map when a layout loads;
// `serializeLayerStyle` reads it back out when one is written.
//
// Only the differences from the indicator's defaults are stored: a layer
// nobody has restyled writes `{}`, and a later change to a default reaches
// every such layer instead of being frozen at whatever it was when the
// layer was created.
//
// Indicator *parameters* (period, fast/slow/signal, k) are not here — those
// live in the layer's own `params` JSON, because changing one recomputes
// the series while changing a colour does not.
import type { LineStyle as LineStyleName } from '$lib/mock/chart-settings';

export interface PlotStyle {
	field: string;
	label: string;
	type: 'line' | 'histogram';
	color: string;
	color2?: string;
	width: number;
	style: LineStyleName;
	visible: boolean;
}

/** `lightweight-charts`' `LineStyle` enum values (Solid=0, Dotted=1,
 * Dashed=2) — kept here rather than importing the library's own enum so
 * this stays a pure data module with no chart-library dependency. */
export const LINE_STYLE_MAP: Record<LineStyleName, number> = { SOLID: 0, DOTTED: 1, DASHED: 2 };

/** One plot per field a given indicator reports (mirrors
 * `crates/api/src/indicator_handlers.rs`'s `reported_fields`/`field_key` —
 * the field keys here are exactly its wire spelling) plus the conventional
 * default color each one drew as an overlay/sub-pane series before this
 * milestone (`$lib/indicators/browser-compute.ts`, now deleted). */
export interface IndicatorDescriptorClient {
	name: string;
	title: string;
	short_title: string;
	legend: string;
	params: { name: string; default: { value: number } }[];
	plots: { field: string; label: string; shape: 'line' | 'histogram'; color: string }[];
	scale: { kind: 'price' | 'ratio' | 'volume' | 'own' };
	placement: 'overlay' | 'sub_pane' | 'either';
}

const descriptors = new Map<string, IndicatorDescriptorClient>();

export function setIndicatorDescriptors(items: IndicatorDescriptorClient[]): void {
	descriptors.clear();
	for (const item of items) descriptors.set(item.name, item);
}

export function indicatorFieldScale(name: string): 'price' | 'qty' | 'none' {
	const kind = descriptors.get(name)?.scale.kind;
	return kind === 'volume' ? 'qty' : kind === 'price' ? 'price' : 'none';
}

function defaultPlotsForIndicator(name: string): PlotStyle[] {
	const descriptor = descriptors.get(name);
	if (descriptor) return descriptor.plots.map((plot) => ({ field: plot.field, label: plot.label, type: plot.shape, color: plot.color, width: 1, style: 'SOLID', visible: true }));
	switch (name) {
		case 'Macd':
			return [
				{ field: 'macd_line', label: 'MACD', type: 'line', color: '#7aa7e8', width: 1, style: 'SOLID', visible: true },
				{ field: 'macd_signal', label: 'SIGNAL', type: 'line', color: '#e8c87a', width: 1, style: 'SOLID', visible: true },
				{ field: 'macd_histogram', label: 'HISTOGRAM', type: 'histogram', color: '#9aa0a6', width: 1, style: 'SOLID', visible: true }
			];
		case 'BollingerBands':
			return [
				{ field: 'bollinger_upper', label: 'UPPER', type: 'line', color: '#7de0a3', width: 1, style: 'SOLID', visible: true },
				{ field: 'bollinger_middle', label: 'MIDDLE', type: 'line', color: '#f2f2ef', width: 1, style: 'SOLID', visible: true },
				{ field: 'bollinger_lower', label: 'LOWER', type: 'line', color: '#e8836f', width: 1, style: 'SOLID', visible: true }
			];
		case 'Stochastic':
			return [
				{ field: 'stochastic_k', label: '%K', type: 'line', color: '#7aa7e8', width: 1, style: 'SOLID', visible: true },
				{ field: 'stochastic_d', label: '%D', type: 'line', color: '#e8c87a', width: 1, style: 'SOLID', visible: true }
			];
		case 'Volume':
			return [{ field: 'value', label: 'PLOT', type: 'histogram', color: '#9aa0a6', width: 1, style: 'SOLID', visible: true }];
		default:
			return [{ field: 'value', label: 'PLOT', type: 'line', color: '#f2f2ef', width: 1, style: 'SOLID', visible: true }];
	}
}

const overrides = new Map<string, Map<string, PlotStyle>>();

/** The plots to render/edit for one layer, seeded from
 * `defaultPlotsForIndicator` and patched with whatever this session has
 * already edited via `setPlotStyle`. */
export function plotsForLayer(layerId: string, indicatorName: string): PlotStyle[] {
	const defaults = defaultPlotsForIndicator(indicatorName);
	const layerOverrides = overrides.get(layerId);
	if (!layerOverrides) return defaults;
	return defaults.map((d) => layerOverrides.get(d.field) ?? d);
}

export function setPlotStyle(layerId: string, indicatorName: string, field: string, patch: Partial<PlotStyle>): void {
	const defaults = defaultPlotsForIndicator(indicatorName);
	let layerOverrides = overrides.get(layerId);
	if (!layerOverrides) {
		layerOverrides = new Map();
		overrides.set(layerId, layerOverrides);
	}
	const current = layerOverrides.get(field) ?? defaults.find((d) => d.field === field);
	if (!current) return;
	layerOverrides.set(field, { ...current, ...patch });
}

/** Fills this layer's overrides from its persisted `style` text. Tolerates
 * `"{}"` and anything malformed: a styling record must never be able to
 * stop a chart from drawing. */
export function hydrateLayerStyle(layerId: string, styleJson: string): void {
	let parsed: unknown;
	try {
		parsed = JSON.parse(styleJson || '{}');
	} catch {
		return;
	}
	if (typeof parsed !== 'object' || parsed === null || Array.isArray(parsed)) return;
	const byField = new Map<string, PlotStyle>();
	for (const [field, value] of Object.entries(parsed as Record<string, unknown>)) {
		if (typeof value === 'object' && value !== null) {
			byField.set(field, value as PlotStyle);
		}
	}
	if (byField.size > 0) overrides.set(layerId, byField);
	else overrides.delete(layerId);
}

/** This layer's overrides as the text the server stores — `"{}"` when
 * nothing has been restyled. */
export function serializeLayerStyle(layerId: string): string {
	const layerOverrides = overrides.get(layerId);
	if (!layerOverrides || layerOverrides.size === 0) return '{}';
	return JSON.stringify(Object.fromEntries(layerOverrides));
}

/** Clears a removed layer's style overrides so this map does not grow
 * unbounded across a long session of adding/removing layers. */
export function clearLayerStyle(layerId: string): void {
	overrides.delete(layerId);
}
