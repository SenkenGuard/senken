// Converts a pane's chart settings to and from `PaneDto.settings`/
// `PaneInputDto.settings` — opaque JSON-object text the server stores and
// returns without interpreting (its own doc: "the chart front end owns what
// these settings mean"). Pulled out of `pane-runtime.ts` as its own module,
// same reasoning as `workspace-error.ts`: a plain function a `bun test` can
// exercise directly, not something that needs the pane/layout shapes around
// it.
//
// The wire shape is a *partial* `ChartSettings` object, not the whole
// interface — a pane that has never had its settings touched carries
// `"{}"` (`PaneInputDto.settings`'s own doc), and a field this build has
// not yet added a control for should not need every existing pane rewritten
// just to gain a key. `parsePaneSettings` merges whatever the text does
// carry onto `defaultChartSettings()`, so a fresh field the dialog does not
// yet expose still resolves to its own default rather than `undefined`.
import { defaultChartSettings, type ChartSettings } from '$lib/mock/chart-settings';

/** Parses `json` into a full `ChartSettings`, tolerating both the documented
 * empty case (`"{}"`) and anything malformed — a pane's settings text is
 * never allowed to break the chart it describes, so a parse failure or a
 * non-object value falls back to `defaultChartSettings()` entirely rather
 * than throwing or handing back a partially-`undefined` object. */
export function parsePaneSettings(json: string): ChartSettings {
	const base = defaultChartSettings();
	let parsed: unknown;
	try {
		parsed = JSON.parse(json);
	} catch {
		// Malformed settings text — the pane's chart still renders with
		// every default rather than being lost along with the bad value.
		return base;
	}
	if (!parsed || typeof parsed !== 'object' || Array.isArray(parsed)) return base;
	const stored = parsed as Record<string, unknown>;
	const merged: Record<string, unknown> = { ...base };
	// Field by field, and only the fields this build has. A blanket spread
	// took the stored text at its word twice over: a key this build no
	// longer has (a setting that was removed because nothing read it) rode
	// along in the object and was written straight back out on the next
	// save, and a value of the wrong type — `marginRight: "6"`, a boolean
	// where a colour belongs — reached the chart as-is. Neither is
	// hypothetical for text that has been through older builds of this app.
	for (const key of Object.keys(base)) {
		const value = stored[key];
		if (value === undefined) continue;
		const expected = merged[key];
		if (Array.isArray(expected)) {
			if (Array.isArray(value) && value.every((entry) => typeof entry === 'number')) {
				merged[key] = value;
			}
			continue;
		}
		if (typeof value === typeof expected) merged[key] = value;
	}
	return merged as unknown as ChartSettings;
}

/** The inverse of `parsePaneSettings` — plain `JSON.stringify`, kept as its
 * own named function so both directions of the codec live next to each
 * other and neither call site has to remember which one of "the whole
 * object" or "just changed keys" the wire format wants (the whole object,
 * always — there is no per-field patch endpoint, same reasoning
 * `pane-runtime.ts`'s `paneToInput` documents for layers). */
export function paneSettingsToJson(settings: ChartSettings): string {
	return JSON.stringify(settings);
}

/**
 * Whether a pane must auto-fit its price axis now, overriding a stored
 * `autoScale: false`.
 *
 * `autoScale: false` means "keep the range I dragged the axis to". The
 * range itself is never persisted, and lightweight-charts v4 has no API to
 * put one back — `IPriceScaleApi` is `applyOptions`/`options`/`width`, and
 * none of them take a price range. So after a reload there is no range to
 * keep, and applying the stored `false` to a series that has no bars yet
 * freezes the scale on an empty chart. The bars then arrive outside it, and
 * the pane reads as blank until it is auto-fitted or reset by hand.
 *
 * All three conditions carry weight:
 * - without bars there is nothing to frame, and fitting an empty series
 *   just freezes a different meaningless range;
 * - only on the series' first bars, so this can never fight a drag the
 *   reader made later in the session;
 * - never when the setting is already auto, because writing a price-scale
 *   option that is already correct discards the manual range.
 */
export function shouldFramePriceScale(state: {
	hasBars: boolean;
	alreadyFramed: boolean;
	storedAutoScale: boolean;
}): boolean {
	return state.hasBars && !state.alreadyFramed && !state.storedAutoScale;
}

/** The stretch factor each pane should hold, main pane first.
 *
 * A weight is a *relative* share the chart re-divides on every resize, so the
 * numbers only have to be proportional to each other. `stored` wins when it
 * has an entry; the fallback splits the remainder evenly between the
 * sub-panes.
 *
 * `subPaneCount` of zero is the case that bites: dividing the remainder by it
 * yields `Infinity`, and a pane given an infinite stretch factor takes the
 * whole chart. It happens for real — every sub-pane indicator can be dropped
 * at once when a fetch comes back with no points. */
export function paneStretchFactors(
	paneCount: number,
	subPaneCount: number,
	stored: number[] | undefined
): number[] {
	if (paneCount <= 0) return [];
	const share = subPaneCount > 0 ? 35 / subPaneCount : 35;
	return Array.from({ length: paneCount }, (_, index) => {
		const configured = stored?.[index];
		const weight = configured ?? (index === 0 ? 65 : share);
		return Number.isFinite(weight) && weight > 0 ? weight : 1;
	});
}
