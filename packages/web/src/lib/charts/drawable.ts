// The one shape a chart pane's object renderer draws — mirrors
// `senken_indicators::Drawable` (`crates/indicators/src/drawable.rs`)
// field-for-field for its bounded, geometric variants: `Segment`, `Level`,
// `Box`, `Label`. `Series` is deliberately not modelled here — an
// indicator's line/histogram output is unbounded in point count and
// already rendered efficiently through `lightweight-charts`' own series
// types (`loadIndicatorSeries` in `./bars.ts`, `chart-pane.svelte`'s
// sub-pane effect); it never needs a hit-test or a handle, so it never
// needs to go through this module.
//
// A persisted drawing (`DrawingRuntime`, `./pane-runtime.ts`) and a future
// indicator-emitted object are different only in *provenance* — where the
// geometry came from. Once either becomes an `ObjectDrawable`, nothing
// downstream (`./drawing-hit.ts`'s pure geometry math, `./drawing-primitive.ts`'s
// paint/hit-test) can tell them apart, and neither should have to: that is
// the single render path this module exists to make real, not just
// aspirational: without one shared shape the renderer gets written twice,
// and the two copies drift.
import type { LineStyle } from '$lib/mock/chart-settings';
import type { DrawingRuntime } from './pane-runtime';

/** How far a segment or level extends beyond its anchor(s) — matches
 * `senken_indicators::Extend` exactly. */
export type Extend = 'none' | 'forward' | 'backward' | 'both';

/** Where a label sits relative to its anchor — matches
 * `senken_indicators::LabelAnchor` exactly. */
export type LabelAnchor = 'above' | 'below' | 'center';

/** A point in a pane's own (time, price) domain — Unix **seconds** on the
 * time axis (matching `DrawingPointRuntime`, not the wire's nanoseconds)
 * and a real price/value on the other. */
export interface DrawablePoint {
	time: number;
	value: number;
}

/** The style a `Segment`/`Level` draws with — one shared scheme for both,
 * one scheme for both: stroke colour, width and line style. */
export interface StrokeStyle {
	color: string;
	width: number;
	lineStyle: LineStyle;
}

/** `Box`'s style: a stroke plus a translucent fill. */
export interface BoxStyle extends StrokeStyle {
	fillOpacity: number;
}

/** `Label`'s style: text colour, background, size — no stroke at all, per
 * a label has no stroke of its own. */
export interface LabelStyle {
	textColor: string;
	background: string;
	fontSize: number;
}

/** The translucent fill every persisted rectangle has always painted with
 * (`drawing-primitive.ts`'s previous, now-removed `paintOne`) — kept as the
 * one default so a rectangle drawn today looks identical after this
 * refactor. */
export const DEFAULT_BOX_FILL_OPACITY = 0.12;

/** The label style an indicator's own `Label` drawable gets until a layer
 * carries its own style override (there is nowhere to store one yet). Also
 * supplies a `text_note` drawing's background/size — the one persisted
 * `Label`, whose own `background`/`fontSize` have nowhere to live yet
 * either; only its `textColor` comes from the drawing's own stored colour
 * (see `drawableFromDrawingRuntime`'s `text_note` case). */
export const DEFAULT_LABEL_STYLE: LabelStyle = {
	textColor: '#0b0d10',
	background: '#f2f2ef',
	fontSize: 11
};

/** The stroke style an indicator's own `Segment`/`Level`/`Box` drawable gets
 * until a layer carries its own style override for them — there is nowhere
 * to store one yet, the same gap `DEFAULT_LABEL_STYLE` documents. Reuses
 * `layer-style.ts`'s own first default plot color (Macd's line, Stochastic's
 * `%K`) rather than inventing a second default palette. */
export const DEFAULT_INDICATOR_STROKE_STYLE: StrokeStyle = {
	color: '#7aa7e8',
	width: 1,
	lineStyle: 'SOLID'
};

/** One drawable object in its pane's own (time, price) domain, tagged with
 * the id of whatever owns it — a persisted drawing's own id, or an
 * indicator layer's id. The renderer never branches on which one it is;
 * only the *owner id* it reports back on a hit distinguishes them (a click
 * on an indicator's `Box` selects the indicator, not a drawing). */
export type ObjectDrawable =
	| { kind: 'level'; ownerId: string; price: number; extend: Extend; style: StrokeStyle }
	| { kind: 'segment'; ownerId: string; a: DrawablePoint; b: DrawablePoint; extend: Extend; style: StrokeStyle }
	| { kind: 'box'; ownerId: string; a: DrawablePoint; b: DrawablePoint; style: BoxStyle }
	| { kind: 'label'; ownerId: string; at: DrawablePoint; text: string; anchor: LabelAnchor; style: LabelStyle };

/** The six standard Fibonacci retracement ratios (0%, 23.6%, 38.2%, 50%,
 * 61.8%, 100%) — `crates/chart/src/store.rs::DrawingKind::FibRetracement`'s
 * own doc names no
 * specific levels (the wire only ever carries the two anchors, `start`/
 * `end`), so this is the one place that picks them: the widely standard
 * six-level set every mainstream charting tool draws between a
 * retracement's two anchors. */
const FIB_RETRACEMENT_RATIOS = [0, 0.236, 0.382, 0.5, 0.618, 1] as const;

/** One fib retracement level's price, linearly interpolated between the
 * drawing's two anchors — `ratio` of the way from `start.price` to
 * `end.price`, so it is correct however the user dragged (high-to-low or
 * low-to-high). */
function fibRetracementPrice(drawing: DrawingRuntime, ratio: number): number {
	const start = drawing.start?.price ?? 0;
	const end = drawing.end?.price ?? 0;
	return start + ratio * (end - start);
}

/** All six of a fib retracement's levels — the actual `Level × 6` fan-out
 * `drawablesFromDrawingRuntime` renders; `drawableFromDrawingRuntime`'s own
 * `fib_retracement` case never runs in practice (see its doc). */
function fibRetracementLevels(drawing: DrawingRuntime, stroke: StrokeStyle): ObjectDrawable[] {
	return FIB_RETRACEMENT_RATIOS.map((ratio) => ({
		kind: 'level' as const,
		ownerId: drawing.id,
		price: fibRetracementPrice(drawing, ratio),
		extend: 'both' as const,
		style: stroke
	}));
}

/** Converts one persisted drawing into the shared `ObjectDrawable` shape —
 * exactly one object, for every kind this can say that about. `senken_chart
 * ::DrawingKind` (`crates/chart/src/store.rs`, schema-owned by a different
 * track, not touched here) now stores six kinds: the original
 * `horizontal_line`/`trend_line`/`rectangle`, plus `ray` (a segment that
 * extends forward), `fib_retracement` (six levels — see below) and
 * `text_note` (a label). A `horizontal_line` always maps to `extend: 'both'`
 * (it has always spanned the full pane), a `trend_line` to `extend: 'none'`
 * and a `ray` to `extend: 'forward'` — all three literally true of every
 * row that kind stores, not a guess about future ones. */
export function drawableFromDrawingRuntime(drawing: DrawingRuntime): ObjectDrawable {
	const stroke: StrokeStyle = { color: drawing.color, width: drawing.width, lineStyle: drawing.lineStyle };
	switch (drawing.kind) {
		case 'horizontal_line':
			return { kind: 'level', ownerId: drawing.id, price: drawing.price ?? 0, extend: 'both', style: stroke };
		case 'fib_retracement':
			// One representative level, for a caller that wants exactly one
			// object back — the retracement's own 50% line. Real rendering
			// never reaches this: `drawablesFromDrawingRuntime` (the only
			// caller `drawing-primitive.ts` uses) intercepts `fib_retracement`
			// first and fans it out to all six levels instead.
			return { kind: 'level', ownerId: drawing.id, price: fibRetracementPrice(drawing, 0.5), extend: 'both', style: stroke };
		case 'text_note':
			return {
				kind: 'label',
				ownerId: drawing.id,
				at: { time: drawing.start?.time ?? 0, value: drawing.start?.price ?? 0 },
				text: drawing.text ?? '',
				anchor: drawing.anchor ?? 'above',
				style: { textColor: drawing.color, background: DEFAULT_LABEL_STYLE.background, fontSize: DEFAULT_LABEL_STYLE.fontSize }
			};
		case 'trend_line':
		case 'rectangle':
		case 'ray': {
			const a: DrawablePoint = { time: drawing.start?.time ?? 0, value: drawing.start?.price ?? 0 };
			const b: DrawablePoint = { time: drawing.end?.time ?? 0, value: drawing.end?.price ?? 0 };
			if (drawing.kind === 'rectangle') {
				return { kind: 'box', ownerId: drawing.id, a, b, style: { ...stroke, fillOpacity: DEFAULT_BOX_FILL_OPACITY } };
			}
			return { kind: 'segment', ownerId: drawing.id, a, b, extend: drawing.kind === 'ray' ? 'forward' : 'none', style: stroke };
		}
	}
}

/** The `ObjectDrawable`(s) one persisted drawing renders as — an array
 * because `fib_retracement` fans out to six `Level`s (015's own tool/anchor
 * mapping, "`Level × 6`"); every other kind is always the single object
 * `drawableFromDrawingRuntime` already produces for it. `drawing-
 * primitive.ts`'s `setDrawings` is the only caller (`drawings.flatMap(...)`
 * in place of the old `drawings.map(drawableFromDrawingRuntime)`) — the one
 * plumbing change a one-drawing-to-many-drawables kind forces, not a
 * per-kind branch in the renderer's own paint/hit-test code, which stays
 * untouched. */
export function drawablesFromDrawingRuntime(drawing: DrawingRuntime): ObjectDrawable[] {
	if (drawing.kind === 'fib_retracement') {
		const stroke: StrokeStyle = { color: drawing.color, width: drawing.width, lineStyle: drawing.lineStyle };
		return fibRetracementLevels(drawing, stroke);
	}
	return [drawableFromDrawingRuntime(drawing)];
}

/** The inverse of `drawableFromDrawingRuntime`, applied after a drag ends —
 * the only geometry fields a persisted `DrawingRuntime` carries. Only ever
 * called on a *persisted* drawable (`drawing-primitive.ts`'s `beginDrag`
 * refuses to start a drag on anything else), so a `label` here is always a
 * `text_note` (`drawableFromDrawingRuntime`'s `text_note` case reuses
 * `start` for its one anchor, `at`) — never an indicator's own display-only
 * label, which can be selected but never dragged in the first place. */
export function patchFromDrawable(drawable: ObjectDrawable): Partial<Pick<DrawingRuntime, 'price' | 'start' | 'end'>> {
	switch (drawable.kind) {
		case 'level':
			return { price: drawable.price };
		case 'segment':
		case 'box':
			return {
				start: { time: drawable.a.time, price: drawable.a.value },
				end: { time: drawable.b.time, price: drawable.b.value }
			};
		case 'label':
			return { start: { time: drawable.at.time, price: drawable.at.value } };
	}
}
