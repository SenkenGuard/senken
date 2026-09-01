// Converts one indicator layer's display list (`POST /api/indicators/compute`'s
// `response.display`, `IndicatorDrawableDto[]`) into the shared `ObjectDrawable`
// shape `./drawable.ts` defines — the *computed* counterpart to
// `drawableFromDrawingRuntime`'s *persisted* one: both produce the exact same
// shape, so `drawing-primitive.ts`'s one renderer/hit-test never has to know
// which side an object came from.
//
// Kept as its own pure module, not inline in `./bars.ts` next to
// `loadIndicatorSeries` (which already unpacks this same `response.display`
// for its `series`-kind entries), for the same reason `source-capability.ts`
// is split from `sources.svelte.ts`: `bars.ts` imports `apiClient`, which
// transitively imports `.svelte.ts` modules that call `$state` at module load
// — so a plain `bun test` cannot import `bars.ts` at all. This module has no
// such import and stays unit-testable directly against the wire shapes
// `crates/indicators/src/drawable.rs` documents.
import type { IndicatorDrawableDto, IndicatorPriceCoordDto } from '$lib/api/types';
import { DEFAULT_BOX_FILL_OPACITY, DEFAULT_INDICATOR_STROKE_STYLE, DEFAULT_LABEL_STYLE, type ObjectDrawable } from './drawable';

/** `IndicatorPointDto.time`/`IndicatorDrawablePointDto.ts_open` are Unix
 * nanoseconds, matching `BarDto`'s own convention; `ObjectDrawable`'s points
 * are Unix seconds, matching `DrawingPointRuntime`. Same conversion
 * `loadIndicatorSeries` already applies to a `series` drawable's points. */
function toSeconds(nanos: number): number {
	return Math.floor(nanos / 1_000_000_000);
}

/** A `level` drawable's price coordinate, resolved to the real number a
 * chart y-position needs. An `executable` anchor's scaled integer is only
 * ever divided back to a float here, at the rendering edge, for *placement*
 * — never reused as a decision value or fed back toward an order (the
 * boundary `AGENTS.md` draws for indicator output). */
function priceCoordToNumber(coord: IndicatorPriceCoordDto): number {
	if (coord.kind === 'annotation') return coord.value;
	return coord.value.value / 10 ** coord.value.scale;
}

/** Converts one layer's `response.display` into the `ObjectDrawable`s that
 * layer contributes to its pane's shared renderer, tagged with `ownerId`
 * (the layer's own id — see `ObjectDrawable`'s own doc on why the renderer
 * never has to branch on provenance). `series` entries are skipped: they are
 * unbounded point series already rendered as a `lightweight-charts` line/
 * histogram (`loadIndicatorSeries`'s `byField`), never a hit-testable object. */
export function objectDrawablesFromIndicatorDisplay(display: IndicatorDrawableDto[], ownerId: string): ObjectDrawable[] {
	const objects: ObjectDrawable[] = [];
	for (const d of display) {
		switch (d.kind) {
			case 'series':
				continue;
			case 'segment':
				objects.push({
					kind: 'segment',
					ownerId,
					a: { time: toSeconds(d.a.time), value: d.a.value },
					b: { time: toSeconds(d.b.time), value: d.b.value },
					extend: d.extend,
					style: DEFAULT_INDICATOR_STROKE_STYLE
				});
				break;
			case 'level':
				objects.push({
					kind: 'level',
					ownerId,
					price: priceCoordToNumber(d.price),
					extend: d.extend,
					style: DEFAULT_INDICATOR_STROKE_STYLE
				});
				break;
			case 'box':
				objects.push({
					kind: 'box',
					ownerId,
					a: { time: toSeconds(d.a.time), value: d.a.value },
					b: { time: toSeconds(d.b.time), value: d.b.value },
					style: { ...DEFAULT_INDICATOR_STROKE_STYLE, fillOpacity: DEFAULT_BOX_FILL_OPACITY }
				});
				break;
			case 'label':
				objects.push({
					kind: 'label',
					ownerId,
					at: { time: toSeconds(d.at.time), value: d.at.value },
					text: d.text,
					anchor: d.anchor,
					style: DEFAULT_LABEL_STYLE
				});
				break;
		}
	}
	return objects;
}
