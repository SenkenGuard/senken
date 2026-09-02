// The one place this app turns a stored instant into text a person reads.
//
// An instant (`crates/core/src/time.rs`'s `UnixNanos`) is UTC and
// timezone-free, and is already correct the moment it is stored. Formatting
// it for a screen is a separate concern with its own failure mode: a chart
// axis, a "Reopens at …" message and a countdown badge can each pick their
// own idea of "which zone", and when they disagree — or one silently falls
// back to whatever the browser happens to report — the same instant reads
// as three different times on the same page.
//
// `formatInstant` (and `formatInstantAcrossZones` below) is the one door
// through which an instant becomes text. Every place in this app that
// renders a time should call one of these instead of building its own
// `Intl.DateTimeFormat` or `Date` formatting.

/** Nanoseconds since the Unix epoch, UTC — the same wire shape
 * `senken_core::UnixNanos` uses on the API. A plain `number`, matching how
 * this value already arrives elsewhere in this app (see
 * `src/lib/charts/live-state.ts`'s own `/ 1_000_000` conversion): a
 * nanosecond-since-epoch magnitude exceeds `Number.MAX_SAFE_INTEGER`, so a
 * round trip through JSON already loses sub-microsecond precision before
 * this module ever sees the value. That loss is harmless for a wall-clock
 * display, which is the only thing this module produces. */
export type InstantNanos = number;

/** A wall-clock rendering of one instant, in one zone.
 *
 * Never a bare string: `zoneId` and `zoneLabel` travel with `text` always,
 * because a time on a screen with no visible zone is ambiguous, and an
 * ambiguous time on a trading screen is worse than one not shown at all. */
export interface RenderedTime {
	/** The formatted wall-clock text, e.g. `'2026-09-01 09:00:00'`. */
	text: string;
	/** The IANA zone id this was rendered in, e.g. `'America/New_York'`. */
	zoneId: string;
	/** A short zone label suitable for display next to `text`, e.g. `'GMT-4'`
	 * — whichever abbreviation or offset the runtime's own zone database
	 * reports as current for `zoneId` at this instant (so it already
	 * reflects whether daylight saving is in effect). */
	zoneLabel: string;
}

/** Options for [`formatInstant`] and [`formatInstantAcrossZones`]. */
export interface FormatInstantOptions {
	/** BCP 47 locale for digit grouping and separators. Defaults to
	 * `'en-US'` rather than the viewer's own browser locale, so two viewers
	 * reading the same instant see the same digits regardless of where each
	 * of them is browsing from. Locale is a legitimate per-viewer
	 * preference this module does not yet expose — fixing it here beats
	 * leaving it to vary by accident. */
	locale?: string;
	/** Whether to include a seconds field. Defaults to `true`. */
	seconds?: boolean;
}

const DEFAULT_LOCALE = 'en-US';

/** Renders `nanos` as wall-clock text in `zoneId`, with that zone's current
 * label attached.
 *
 * `zoneId` controls only how `nanos` is *written down* — never which
 * instant is being described. Calling this twice with the same `nanos` and
 * two different zones must never change `nanos` itself, only `text` and
 * `zoneLabel`; see this module's tests for the property this guarantees.
 *
 * DST is handled by the runtime's own time zone database (the same one
 * `Intl` uses everywhere else): an ambiguous or skipped wall-clock hour
 * cannot arise here the way it can on the *input* side, because every
 * instant has exactly one wall-clock representation in a given zone. */
export function formatInstant(
	nanos: InstantNanos,
	zoneId: string,
	options: FormatInstantOptions = {}
): RenderedTime {
	const date = new Date(Math.trunc(nanos / 1_000_000));
	const locale = options.locale ?? DEFAULT_LOCALE;
	const seconds = options.seconds ?? true;

	const parts = Object.fromEntries(
		new Intl.DateTimeFormat(locale, {
			timeZone: zoneId,
			year: 'numeric',
			month: '2-digit',
			day: '2-digit',
			hour: '2-digit',
			minute: '2-digit',
			second: seconds ? '2-digit' : undefined,
			hour12: false
		})
			.formatToParts(date)
			.map((part) => [part.type, part.value])
	);
	const text = seconds
		? `${parts.year}-${parts.month}-${parts.day} ${parts.hour}:${parts.minute}:${parts.second}`
		: `${parts.year}-${parts.month}-${parts.day} ${parts.hour}:${parts.minute}`;

	const zoneLabel =
		new Intl.DateTimeFormat(locale, { timeZone: zoneId, timeZoneName: 'shortOffset' })
			.formatToParts(date)
			.find((part) => part.type === 'timeZoneName')?.value ?? zoneId;

	return { text, zoneId, zoneLabel };
}

/** One instant, rendered in a primary zone plus zero or more secondary
 * ones — the shape a chart showing "venue time alongside my time" needs.
 *
 * Exactly one zone is primary. Two zones with equal billing is the same as
 * neither being authoritative: a caller displaying more than one zone (a
 * chart's time axis, say) must still say which one drives layout, and
 * `primary` is that one. */
export interface MultiZoneRendering {
	primary: RenderedTime;
	secondary: RenderedTime[];
}

/** Renders `nanos` in `primaryZoneId` and, if given, every zone in
 * `secondaryZoneIds` — all sharing the same `options`. See
 * [`MultiZoneRendering`] for why exactly one of the results is `primary`. */
export function formatInstantAcrossZones(
	nanos: InstantNanos,
	primaryZoneId: string,
	secondaryZoneIds: readonly string[] = [],
	options?: FormatInstantOptions
): MultiZoneRendering {
	return {
		primary: formatInstant(nanos, primaryZoneId, options),
		secondary: secondaryZoneIds.map((zoneId) => formatInstant(nanos, zoneId, options))
	};
}

/** The individual wall-clock components of one instant in one zone, for a
 * caller that needs to assemble its own display string instead of the
 * fixed `'YYYY-MM-DD HH:MM:SS'` shape [`formatInstant`] returns — a chart's
 * time axis, say, which shows only a weekday and a compact date on one
 * tick and only a clock on the next. The pieces still come from this
 * module's own `Intl.DateTimeFormat(..., { timeZone: zoneId })` calls, so a
 * caller building a custom layout from them never reaches for its own
 * unzoned `Date` formatting to do it. */
export interface ZonedTimeParts {
	/** Short weekday, e.g. `'Mon'`. */
	weekday: string;
	/** Day and month, e.g. `'Sep 1'`. */
	monthDay: string;
	/** Hour and minute, 24-hour clock, e.g. `'09:00'`. */
	clock24: string;
	/** Hour and minute, 12-hour clock with AM/PM, e.g. `'9:00 AM'`. */
	clock12: string;
}

/** Splits `nanos` into the individual pieces [`ZonedTimeParts`] documents,
 * rendered in `zoneId`. Same zone-handling as [`formatInstant`] — DST and
 * the zone's own current offset are resolved by the runtime's zone
 * database, never guessed here. */
export function zonedTimeParts(
	nanos: InstantNanos,
	zoneId: string,
	locale: string = DEFAULT_LOCALE
): ZonedTimeParts {
	const date = new Date(Math.trunc(nanos / 1_000_000));
	const format = (options: Intl.DateTimeFormatOptions): string =>
		new Intl.DateTimeFormat(locale, { timeZone: zoneId, ...options }).format(date);
	return {
		weekday: format({ weekday: 'short' }),
		monthDay: format({ day: 'numeric', month: 'short' }),
		clock24: format({ hour: '2-digit', minute: '2-digit', hour12: false }),
		clock12: format({ hour: '2-digit', minute: '2-digit', hour12: true })
	};
}
