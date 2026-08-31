// Converts between the server's workspace/layout wire shapes
// (`$lib/api/types.ts`'s `LayoutDetailDto`/`PaneDto`/`LayerDto`, generated
// from `senken_workspace::{LayoutDetail,PaneRecord,LayerRecord}`) and a
// small runtime shape the charts page's components render from. Unlike the former mock model (`$lib/mock/charts.ts`'s `PaneLayer`),
// a pane's *main* instrument/timeframe are the pane's own fields, not a
// "primary" layer — `senken_workspace::LayerKind` only ever models a
// *second* overlaid instrument or an indicator, matching
// `crates/workspace/src/store.rs::PaneRecord` exactly.
import type {
	DrawingDto,
	DrawingInputDto,
	DrawingKindDto,
	LayerDto,
	LayerInputDto,
	LayerKindDto,
	PaneDto,
	PaneInputDto,
	ReplaceLayoutRequest
} from '$lib/api/types';
import type { ChartSettings, LineStyle } from '$lib/mock/chart-settings';
import { hydrateLayerStyle, serializeLayerStyle } from './layer-style';
import type { LayerKindTag } from '$lib/components/terminal/chart-config';
import { parsePaneSettings, paneSettingsToJson } from './pane-settings';

export interface LayerRuntime {
	id: string;
	position: number;
	kind: LayerKindTag;
	visible: boolean;
	/** `overlay_instrument` only. */
	instrument?: string;
	/** `indicator_overlay`/`indicator_sub_pane` only — matches exactly what
	 * `GET /api/indicators` catalogues (e.g. `"Rsi"`, `"BollingerBands"`). */
	indicatorName?: string;
	/** Parsed from the layer's opaque `params` JSON text — the indicator's
	 * construction parameters (e.g. `{ period: 14 }`), never style. There is
	 * no server field for a layer's color/width/line-style yet (the "if its layout endpoint cannot yet carry a field you need,
	 * say which"): those stay local-only,
	 * keyed by `id` in `$lib/charts/layer-style.ts`. */
	params: Record<string, number>;
}

/** What kind of drawing object this is — the minimum viable set (a
 * horizontal line, a trend line, a rectangle zone), matching
 * `senken_workspace::DrawingKind` exactly. */
export type DrawingKindTag = 'horizontal_line' | 'trend_line' | 'rectangle';

/** One (time, price) anchor for a `trend_line`/`rectangle` drawing.
 * `time` is Unix **seconds** (a `lightweight-charts` `UTCTimestamp`,
 * matching how `chart-pane.svelte` already turns a bar's nanosecond
 * `ts_open` into chart time) — `drawingFromDto`/`drawingToInput` are the
 * only two places that convert to and from the wire's nanoseconds. */
export interface DrawingPointRuntime {
	time: number;
	price: number;
}

/** A drawing object: identity, geometry and the one style every drawing
 * carries (colour/width/line-style) — real, persisted objects
 * (`$lib/charts/drawing-primitive.ts` renders and hit-tests them), not the
 * click-to-paint price lines `chart-pane.svelte` used to draw. */
export interface DrawingRuntime {
	id: string;
	position: number;
	kind: DrawingKindTag;
	/** `horizontal_line` only. */
	price?: number;
	/** `trend_line`/`rectangle` only. */
	start?: DrawingPointRuntime;
	/** `trend_line`/`rectangle` only. */
	end?: DrawingPointRuntime;
	color: string;
	width: number;
	lineStyle: LineStyle;
}

export function drawingFromDto(dto: DrawingDto): DrawingRuntime {
	const base = {
		id: dto.id,
		position: dto.position,
		color: dto.color,
		width: dto.width,
		lineStyle: dto.line_style
	};
	const kind = dto.kind;
	if (kind.kind === 'horizontal_line') {
		return { ...base, kind: 'horizontal_line', price: kind.price };
	}
	return {
		...base,
		kind: kind.kind,
		start: { time: Math.floor(kind.start.time / 1_000_000_000), price: kind.start.price },
		end: { time: Math.floor(kind.end.time / 1_000_000_000), price: kind.end.price }
	};
}

export function drawingToInput(drawing: DrawingRuntime): DrawingInputDto {
	const kind: DrawingKindDto =
		drawing.kind === 'horizontal_line'
			? { kind: 'horizontal_line', price: drawing.price ?? 0 }
			: {
					kind: drawing.kind,
					start: { time: (drawing.start?.time ?? 0) * 1_000_000_000, price: drawing.start?.price ?? 0 },
					end: { time: (drawing.end?.time ?? 0) * 1_000_000_000, price: drawing.end?.price ?? 0 }
				};
	return {
		position: drawing.position,
		kind,
		color: drawing.color,
		width: drawing.width,
		line_style: drawing.lineStyle
	};
}

export interface PaneRuntime {
	id: string;
	position: number;
	instrument: string;
	timeframe: string;
	layers: LayerRuntime[];
	drawings: DrawingRuntime[];
	/** This pane's chart settings (candle colours, status line, scales,
	 * canvas, trading — `terminal/chart-settings-dialog.svelte`), parsed
	 * from the server's opaque `settings` JSON text (`parsePaneSettings`).
	 * Kept local-only for a long time and lost on every reload precisely
	 * because there was nowhere on `PaneDto` to persist it — this field is
	 * what closes that gap. */
	settings: ChartSettings;
}

function parseParams(json: string): Record<string, number> {
	try {
		const value: unknown = JSON.parse(json);
		if (value && typeof value === 'object' && !Array.isArray(value)) {
			return value as Record<string, number>;
		}
	} catch {
		// malformed params text — treat as empty rather than throwing away
		// the whole layer.
	}
	return {};
}

export function layerFromDto(dto: LayerDto): LayerRuntime {
	hydrateLayerStyle(dto.id, dto.style);
	const kind = dto.kind;
	if (kind.kind === 'overlay_instrument') {
		return { id: dto.id, position: dto.position, kind: 'overlay_instrument', visible: dto.visible, instrument: kind.instrument, params: {} };
	}
	return {
		id: dto.id,
		position: dto.position,
		kind: kind.kind,
		visible: dto.visible,
		indicatorName: kind.name,
		params: parseParams(kind.params)
	};
}

export function layerToInput(layer: LayerRuntime): LayerInputDto {
	const kind: LayerKindDto =
		layer.kind === 'overlay_instrument'
			? { kind: 'overlay_instrument', instrument: layer.instrument ?? '' }
			: { kind: layer.kind, name: layer.indicatorName ?? '', params: JSON.stringify(layer.params) };
	return {
		position: layer.position,
		kind,
		visible: layer.visible,
		style: serializeLayerStyle(layer.id)
	};
}

export function paneFromDto(dto: PaneDto): PaneRuntime {
	return {
		id: dto.id,
		position: dto.position,
		instrument: dto.instrument,
		timeframe: dto.timeframe,
		layers: dto.layers.map(layerFromDto).sort((a, b) => a.position - b.position),
		drawings: dto.drawings.map(drawingFromDto).sort((a, b) => a.position - b.position),
		settings: parsePaneSettings(dto.settings)
	};
}

export function paneToInput(pane: PaneRuntime): PaneInputDto {
	return {
		position: pane.position,
		instrument: pane.instrument,
		timeframe: pane.timeframe,
		layers: pane.layers.map(layerToInput),
		drawings: pane.drawings.map(drawingToInput),
		settings: paneSettingsToJson(pane.settings)
	};
}

export interface TreeGroup {
	title: string;
	items: { name: string; params: string; visible: boolean }[];
}

function layerDisplayName(layer: LayerRuntime): string {
	if (layer.kind === 'overlay_instrument') return layer.instrument ?? '';
	return layer.indicatorName ?? '';
}

function layerParamsLabel(layer: LayerRuntime): string {
	if (layer.kind === 'overlay_instrument') return 'overlay';
	const entries = Object.entries(layer.params);
	return entries.length === 0 ? '' : entries.map(([k, v]) => `${k}: ${v}`).join(' · ');
}

function drawingDisplayName(kind: DrawingKindTag): string {
	switch (kind) {
		case 'horizontal_line':
			return 'HORIZONTAL LINE';
		case 'trend_line':
			return 'TREND LINE';
		case 'rectangle':
			return 'RECTANGLE';
	}
}

function drawingParamsLabel(drawing: DrawingRuntime): string {
	if (drawing.kind === 'horizontal_line') return (drawing.price ?? 0).toFixed(2);
	const start = drawing.start;
	const end = drawing.end;
	if (!start || !end) return '';
	return `${start.price.toFixed(2)} → ${end.price.toFixed(2)}`;
}

/** Object-tree groups for the right panel's OBJECT TREE tab: this pane's
 * layers grouped by kind, plus a DRAWINGS group listing its real drawing
 * objects — always shown as `visible: true` since a drawing has no
 * visibility toggle of its own (unlike a layer's), only colour/width/
 * style. */
export function objectTree(layers: LayerRuntime[], drawings: DrawingRuntime[] = []): TreeGroup[] {
	const groups: { kind: LayerKindTag[]; title: string }[] = [
		{ kind: ['overlay_instrument'], title: 'INSTRUMENTS' },
		{ kind: ['indicator_overlay', 'indicator_sub_pane'], title: 'INDICATORS' }
	];
	return groups
		.map((g) => ({
			title: g.title,
			items: layers
				.filter((l) => g.kind.includes(l.kind))
				.map((l) => ({ name: layerDisplayName(l), params: layerParamsLabel(l), visible: l.visible }))
		}))
		.concat([
			{
				title: 'DRAWINGS',
				items: drawings.map((d) => ({
					name: drawingDisplayName(d.kind),
					params: drawingParamsLabel(d),
					visible: true
				}))
			}
		]);
}

/** Splits one pane's layers into the ones that render over the main price
 * chart and the ones that get their own pane below it — mirrors the former
 * mock's `splitPaneLayers`, on the new runtime shape. */
export function splitPaneLayers(layers: LayerRuntime[]): { main: LayerRuntime[]; sub: LayerRuntime[] } {
	const sub = layers.filter((l) => l.kind === 'indicator_sub_pane');
	const main = layers.filter((l) => l.kind !== 'indicator_sub_pane');
	return { main, sub };
}

/** Builds the whole-layout request `PUT /api/layouts/{id}` needs — there is
 * no per-pane or per-layer patch endpoint (`WorkspaceStore` itself has none;
 *'s implementation notes), so every mutation the charts
 * page makes replaces the full pane/layer structure in one call. */
export function toReplaceLayoutRequest(preset: string, panes: PaneRuntime[]): ReplaceLayoutRequest {
	return { preset, panes: panes.map(paneToInput) };
}
