// Converts between the server's chart wire shapes
// (`$lib/api/types.ts`'s `LayoutDetailDto`/`PaneDto`/`LayerDto`/`DrawingDto`,
// generated from the API's DTO layer over `senken_chart::{ChartLayoutDetail,
// PaneRecord, PaneItemRecord}`) and a small runtime shape the charts page's
// components render from. Unlike the former mock model (`$lib/mock/charts.ts`'s
// `PaneLayer`), a pane's *main* instrument/timeframe are the pane's own
// fields, not a "primary" layer — the store's own `ItemSource`
// (`crates/chart/src/store.rs`) only ever models a *second* overlaid
// instrument, a computed indicator, or an anchored drawing, matching
// `PaneRecord`/`PaneItemRecord` exactly. The wire still splits those into
// separate `layers`/`drawings` arrays (`LayerDto`/`DrawingDto`) rather than
// `PaneItemRecord`'s one unified list, which is why this module keeps that
// same split.
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
import type { LabelAnchor } from './drawable';
import { hydrateLayerStyle, serializeLayerStyle } from './layer-style';
import type { LayerKindTag, ToolKey } from '$lib/components/terminal/chart-config';
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

/** What kind of drawing object this is, matching `senken_chart::DrawingKind`
 * exactly: the original `horizontal_line`/`trend_line`/`rectangle`, plus
 * `ray` (a segment that extends forward), `fib_retracement` (renders as six
 * levels — see `$lib/charts/drawable.ts`'s `drawablesFromDrawingRuntime`)
 * and `text_note` (a label). */
export type DrawingKindTag = 'horizontal_line' | 'trend_line' | 'rectangle' | 'ray' | 'fib_retracement' | 'text_note';

/** One (time, price) anchor for a multi-point drawing (`trend_line`/
 * `rectangle`/`ray`/`fib_retracement`'s `start`/`end`, `text_note`'s `at`).
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
	/** Whether the drawing is currently shown — a real, server-stored field
	 * since schema v8 (`DrawingDto.visible`), not a client-side guess. */
	visible: boolean;
	/** `horizontal_line` only. */
	price?: number;
	/** `trend_line`/`rectangle`/`ray`/`fib_retracement`'s two anchors, or
	 * `text_note`'s one (`at` on the wire — reused here rather than adding a
	 * fourth optional point field for a kind that only ever needs one). */
	start?: DrawingPointRuntime;
	/** `trend_line`/`rectangle`/`ray`/`fib_retracement` only. */
	end?: DrawingPointRuntime;
	/** `text_note` only. */
	text?: string;
	/** `text_note` only. */
	anchor?: LabelAnchor;
	/** The instrument this drawing was drawn against, `source:symbol`.
	 *
	 * A drawing's anchors are prices and instants on one particular market:
	 * a trend line across Bitcoin's range says nothing about Ethereum, and
	 * drawing it over Ethereum's candles because the same pane happens to be
	 * showing them is a claim nobody made. `undefined` for a drawing stored
	 * before the server carried this field — see `drawingsForInstrument`. */
	instrument?: string;
	color: string;
	width: number;
	lineStyle: LineStyle;
}

/** How many chart clicks a draw tool needs before it acts. A tool anchored
 * to a single (time, price) click — a horizontal line (price only) or a
 * text note (both) — finishes on the first click; every other live tool
 * needs two. This is the one place `chart-pane.svelte`'s click handler asks
 * "how many anchors does this tool have" — a table lookup instead of a
 * chain of `tool === '...'` checks per tool. */
export const TOOL_ANCHOR_COUNT: Partial<Record<ToolKey, 1 | 2>> = {
	hline: 1,
	text: 1,
	trend: 2,
	rect: 2,
	ray: 2,
	fib: 2,
	measure: 2
};

/** The `DrawingKindTag` a finished tool becomes, for the tools that persist
 * one at all. `measure` has two anchors (`TOOL_ANCHOR_COUNT`) but no entry
 * here — its second click never calls `onCreateDrawing`, only
 * `$lib/charts/measure.ts`'s pure stats. `chart-pane.svelte`'s click
 * handler treats "no kind for this tool" as the one condition that means
 * "transient, not persisted", rather than naming `measure` itself. */
export const DRAWING_KIND_FOR_TOOL: Partial<Record<ToolKey, DrawingKindTag>> = {
	hline: 'horizontal_line',
	trend: 'trend_line',
	rect: 'rectangle',
	ray: 'ray',
	fib: 'fib_retracement',
	text: 'text_note'
};

export function drawingFromDto(dto: DrawingDto): DrawingRuntime {
	const base = {
		id: dto.id,
		position: dto.position,
		visible: dto.visible,
		instrument: dto.instrument ?? undefined,
		color: dto.color,
		width: dto.width,
		lineStyle: dto.line_style
	};
	const kind = dto.kind;
	if (kind.kind === 'horizontal_line') {
		return { ...base, kind: 'horizontal_line', price: kind.price };
	}
	if (kind.kind === 'text_note') {
		return {
			...base,
			kind: 'text_note',
			start: { time: Math.floor(kind.at.time / 1_000_000_000), price: kind.at.price },
			text: kind.text,
			anchor: kind.anchor
		};
	}
	return {
		...base,
		kind: kind.kind,
		start: { time: Math.floor(kind.start.time / 1_000_000_000), price: kind.start.price },
		end: { time: Math.floor(kind.end.time / 1_000_000_000), price: kind.end.price }
	};
}

export function drawingToInput(drawing: DrawingRuntime): DrawingInputDto {
	let kind: DrawingKindDto;
	if (drawing.kind === 'horizontal_line') {
		kind = { kind: 'horizontal_line', price: drawing.price ?? 0 };
	} else if (drawing.kind === 'text_note') {
		kind = {
			kind: 'text_note',
			at: { time: (drawing.start?.time ?? 0) * 1_000_000_000, price: drawing.start?.price ?? 0 },
			text: drawing.text ?? '',
			anchor: drawing.anchor ?? 'above'
		};
	} else {
		kind = {
			kind: drawing.kind,
			start: { time: (drawing.start?.time ?? 0) * 1_000_000_000, price: drawing.start?.price ?? 0 },
			end: { time: (drawing.end?.time ?? 0) * 1_000_000_000, price: drawing.end?.price ?? 0 }
		};
	}
	return {
		position: drawing.position,
		kind,
		visible: drawing.visible,
		instrument: drawing.instrument ?? null,
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

/** What an `ObjectTreeItem` actually is — the object tree panel needs this to
 * know which store mutation a row's controls should call (`toggleLayerVisible`
 * vs `toggleDrawingVisible`, `removeLayer` vs `removeDrawing`, …), which the
 * former nameless `{ name, params, visible }` row could not express at all. */
export type ObjectTreeKind = 'layer' | 'drawing';

/** One row in the OBJECT TREE panel: identity (`id`/`kind`, so a click can act
 * on the right underlying layer or drawing), display text, and whether it can
 * be shown/hidden/reordered/removed and opens a settings dialog. */
export interface ObjectTreeItem {
	id: string;
	kind: ObjectTreeKind;
	/** Display name, e.g. "EMA", "BINANCE-SPOT:ETHUSDT", "TREND LINE". */
	name: string;
	/** Secondary line: params, price, "overlay". */
	params: string;
	visible: boolean;
	/** Whether this row can open a settings dialog — an `overlay_instrument`
	 * layer has none (there is no dialog for a second instrument, only for an
	 * indicator's parameters or a drawing's style), so it alone is `false`. */
	canConfigure: boolean;
}

export interface ObjectTreeGroup {
	key: string;
	title: string;
	items: ObjectTreeItem[];
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
		case 'ray':
			return 'RAY';
		case 'fib_retracement':
			return 'FIB RETRACEMENT';
		case 'text_note':
			return 'TEXT NOTE';
	}
}

function drawingParamsLabel(drawing: DrawingRuntime): string {
	if (drawing.kind === 'horizontal_line') return (drawing.price ?? 0).toFixed(2);
	if (drawing.kind === 'text_note') return drawing.text ?? '';
	const start = drawing.start;
	const end = drawing.end;
	if (!start || !end) return '';
	return `${start.price.toFixed(2)} → ${end.price.toFixed(2)}`;
}

/** Object-tree groups for the right panel's OBJECT TREE tab: this pane's
 * layers grouped by kind, plus a DRAWINGS group listing its real drawing
 * objects, using each drawing's own `visible` flag the same way a layer's
 * is used above — real since schema v8 (`DrawingDto.visible`), not the
 * hardcoded `true` this used to fall back to before a drawing could store
 * one at all. Rows come out in the same `position`-ascending order the chart
 * draws in (`layers`/`drawings` are already sorted that way by
 * `paneFromDto`), so "move up"/"move down" in the panel matches what moves
 * in front of what on the chart. */
export function objectTree(layers: LayerRuntime[], drawings: DrawingRuntime[] = []): ObjectTreeGroup[] {
	const groups: { key: string; kind: LayerKindTag[]; title: string }[] = [
		{ key: 'instruments', kind: ['overlay_instrument'], title: 'INSTRUMENTS' },
		{ key: 'indicators', kind: ['indicator_overlay', 'indicator_sub_pane'], title: 'INDICATORS' }
	];
	const layerGroups: ObjectTreeGroup[] = groups.map((g) => ({
		key: g.key,
		title: g.title,
		items: layers
			.filter((l) => g.kind.includes(l.kind))
			.map((l) => ({
				id: l.id,
				kind: 'layer' as const,
				name: layerDisplayName(l),
				params: layerParamsLabel(l),
				visible: l.visible,
				canConfigure: l.kind !== 'overlay_instrument'
			}))
	}));
	return layerGroups.concat([
			{
				key: 'drawings',
				title: 'DRAWINGS',
				items: drawings.map((d) => ({
					id: d.id,
					kind: 'drawing' as const,
					name: drawingDisplayName(d.kind),
					params: drawingParamsLabel(d),
					visible: d.visible,
					canConfigure: true
				}))
			}
		]);
}

/** Which id `id` should swap places with inside `orderedIds` if it moved
 * `direction` — the OBJECT TREE panel's own group order (INSTRUMENTS/
 * INDICATORS/DRAWINGS are separate groups; `orderedIds` is always one
 * group's own ids, never the pane's whole `layers`/`drawings` array), so a
 * move can never reach past the row at either end of the group. `null` when
 * `id` is already at that end, or is not in `orderedIds` at all. */
export function neighborInGroup(orderedIds: string[], id: string, direction: 'up' | 'down'): string | null {
	const index = orderedIds.indexOf(id);
	if (index === -1) return null;
	const neighborIndex = direction === 'up' ? index - 1 : index + 1;
	return orderedIds[neighborIndex] ?? null;
}

/** Swaps the two items named `aId`/`bId` and renumbers every item's
 * `position` to match the new order — the pure part of
 * `swapLayerOrder`/`swapDrawingOrder` (`workspace-store.svelte.ts`), split
 * out so it is testable without a live store. `items` need not already be
 * sorted; the result always is. Returns `items` unchanged if either id is
 * missing or the two ids are the same (nothing to swap). */
export function swapById<T extends { id: string; position: number }>(items: T[], aId: string, bId: string): T[] {
	const sorted = [...items].sort((a, b) => a.position - b.position);
	const aIndex = sorted.findIndex((item) => item.id === aId);
	const bIndex = sorted.findIndex((item) => item.id === bId);
	if (aIndex === -1 || bIndex === -1 || aIndex === bIndex) return items;
	[sorted[aIndex], sorted[bIndex]] = [sorted[bIndex], sorted[aIndex]];
	return sorted.map((item, i) => ({ ...item, position: i }));
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
 * no per-pane or per-layer patch endpoint (`senken_chart::ChartWorkspaceStore`
 * itself has none), so every mutation the charts page makes replaces the
 * full pane/layer structure in one call. */
export function toReplaceLayoutRequest(preset: string, panes: PaneRuntime[]): ReplaceLayoutRequest {
	return { preset, panes: panes.map(paneToInput) };
}

/** The drawings a pane showing `instrument` should render.
 *
 * A pane keeps every drawing ever made on it, across every instrument it has
 * shown — switching symbol and back brings the old ones straight back, which
 * is what a reader expects and what every charting tool does. What it must
 * never do is paint one instrument's levels over another's candles: the
 * anchors are prices on a market, and a horizontal line at 2450 means
 * nothing on a market trading at 0.4.
 *
 * A drawing with no recorded instrument is shown whatever the pane displays.
 * That is the honest reading of "nothing ever recorded what this was drawn
 * against" — hiding it would silently lose work, and attributing it to
 * whatever happens to be on screen would invent a fact. Only drawings stored
 * before the server carried the field are in that state; every new one
 * records it. */
export function drawingsForInstrument(
	drawings: DrawingRuntime[],
	instrument: string
): DrawingRuntime[] {
	return drawings.filter((drawing) => !drawing.instrument || drawing.instrument === instrument);
}
