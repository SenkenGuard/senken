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
	try {
		const parsed: unknown = JSON.parse(json);
		if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
			return { ...base, ...(parsed as Partial<ChartSettings>) };
		}
	} catch {
		// Malformed settings text — the pane's chart still renders with
		// every default rather than being lost along with the bad value.
	}
	return base;
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
