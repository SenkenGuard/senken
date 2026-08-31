//! Encodes the `{origin}-{spec}` directory segment used under a series'
//! `bars/` tree, extended to carry a
//! Day-or-above series' anchor when it is not UTC.
//!
//! F3, measured live against OKX: `bar=1D` opens at UTC+8 while `bar=1Dutc`
//! opens at UTC — the same nominal spec, two different series. Persisting
//! both under one `venue-1d` directory would silently interleave bars
//! eight hours apart, the same class of bug already paid for once with
//! Phemex's `sOLUSDT`/`SOLUSDT` collision. So for [`BarUnit::Day`] and
//! coarser, a non-UTC anchor is appended to the directory name
//! (`venue-1d@utc8`); below `Day` the anchor is meaningless (an hour
//! boundary needs no notion of "midnight") and is never encoded, matching
//! [`Anchor`]'s own documented scope.
//!
//! No Arrow/Parquet dependency — this is string encoding over
//! `senken-series` types only.

use std::str::FromStr;

use senken_series::{Anchor, BarSpec, BarUnit, Origin};

const NANOS_PER_HOUR: i64 = 3_600_000_000_000;
const NANOS_PER_MINUTE: i64 = 60_000_000_000;

/// `true` for the units [`Anchor`] actually applies to. Shared with
/// `assertions`, which needs the same day-or-above test to decide
/// whether a misaligned bar is a plain alignment defect or an anchor
/// mismatch.
pub(crate) fn anchor_applies_to(unit: BarUnit) -> bool {
    // `BarUnit` is `#[non_exhaustive]`: a future non-calendar unit
    // (volume/tick/Renko bars, per that type's own docs) has no notion of
    // "midnight" either, so it falls to the same `false` the sub-day units
    // get, via `matches!`'s implicit wildcard arm.
    matches!(unit, BarUnit::Day | BarUnit::Week | BarUnit::Month)
}

/// The anchor suffix appended after `@`, or `""` when none is needed.
///
/// The number after `utc` is the venue's own UTC offset in the ordinary
/// human sense — `utc8` reads as "this series' day rolls over at UTC+8
/// (Hong Kong) midnight", matching how F3 itself describes OKX. That is
/// the *negation* of [`Anchor::offset_nanos`]: a venue ahead of UTC rolls
/// its day over *before* UTC midnight arrives, which is
/// [`Anchor::from_offset_nanos`]'s documented *negative* case. Getting
/// this backwards would round-trip perfectly (encode and decode would
/// still agree with each other) while silently mislabelling every such
/// directory for a human reading it — exactly the kind of quiet mistake
/// this encoding exists to prevent, just moved one level up.
///
/// Whole hours render as `utc{h}` (matching the plan's own example,
/// `venue-1d@utc8`); a non-hour-aligned offset — not observed at any real
/// venue so far, but `Anchor` does not forbid it — falls back to whole
/// minutes (`utcm{m}`) and finally raw nanoseconds (`utcn{n}`), so the
/// encoding never silently loses precision regardless of what `Anchor`
/// value it is given.
fn encode_anchor_suffix(unit: BarUnit, anchor: Anchor) -> String {
    if !anchor_applies_to(unit) || anchor == Anchor::UTC {
        return String::new();
    }
    // Negate: see this function's doc comment for why.
    let venue_offset = -anchor.offset_nanos();
    if venue_offset % NANOS_PER_HOUR == 0 {
        format!("@utc{}", venue_offset / NANOS_PER_HOUR)
    } else if venue_offset % NANOS_PER_MINUTE == 0 {
        format!("@utcm{}", venue_offset / NANOS_PER_MINUTE)
    } else {
        format!("@utcn{venue_offset}")
    }
}

/// Inverse of [`encode_anchor_suffix`]'s non-empty case: parses the part
/// after `@` (not including it), negating back to [`Anchor`]'s own sign
/// convention. Order matters — `utcn`/`utcm` must be tried before the bare
/// `utc` prefix, since `utc` is itself a prefix of both.
fn decode_anchor_suffix(s: &str) -> Option<Anchor> {
    if let Some(rest) = s.strip_prefix("utcn") {
        let venue_offset: i64 = rest.parse().ok()?;
        return Some(Anchor::from_offset_nanos(venue_offset.checked_neg()?));
    }
    if let Some(rest) = s.strip_prefix("utcm") {
        let minutes: i64 = rest.parse().ok()?;
        let venue_offset = minutes.checked_mul(NANOS_PER_MINUTE)?;
        return Some(Anchor::from_offset_nanos(venue_offset.checked_neg()?));
    }
    if let Some(rest) = s.strip_prefix("utc") {
        let hours: i64 = rest.parse().ok()?;
        let venue_offset = hours.checked_mul(NANOS_PER_HOUR)?;
        return Some(Anchor::from_offset_nanos(venue_offset.checked_neg()?));
    }
    None
}

/// Encodes `spec`'s directory token, including its anchor suffix when one
/// applies and is not UTC — e.g. `1m`, `1d`, `1d@utc8`.
#[must_use]
pub fn encode_spec_token(spec: BarSpec, anchor: Anchor) -> String {
    format!("{spec}{}", encode_anchor_suffix(spec.unit, anchor))
}

/// Inverse of [`encode_spec_token`]. A spec with no `@` suffix decodes to
/// [`Anchor::UTC`].
#[must_use]
pub fn decode_spec_token(token: &str) -> Option<(BarSpec, Anchor)> {
    if let Some((spec_part, anchor_part)) = token.split_once('@') {
        let spec = BarSpec::from_str(spec_part).ok()?;
        let anchor = decode_anchor_suffix(anchor_part)?;
        Some((spec, anchor))
    } else {
        let spec = BarSpec::from_str(token).ok()?;
        Some((spec, Anchor::UTC))
    }
}

/// The full `bars/` subdirectory name: `{origin}-{spec_token}`.
///
/// `pub(crate)`, not exported: [`crate::paths`] is the public surface for
/// path construction, and re-exposing this too would just be two ways to
/// build the same string.
#[must_use]
pub(crate) fn encode_bars_dir_name(origin: Origin, spec: BarSpec, anchor: Anchor) -> String {
    format!("{origin}-{}", encode_spec_token(spec, anchor))
}

/// Inverse of [`encode_bars_dir_name`]. Splits on the first `-`, which is
/// safe because neither [`Origin::Display`] (`"venue"`/`"derived"`) nor a
/// [`BarSpec`] token ever contains one.
#[must_use]
pub(crate) fn decode_bars_dir_name(name: &str) -> Option<(Origin, BarSpec, Anchor)> {
    let (origin_part, spec_part) = name.split_once('-')?;
    let origin = Origin::from_str(origin_part).ok()?;
    let (spec, anchor) = decode_spec_token(spec_part)?;
    Some((origin, spec, anchor))
}

#[cfg(test)]
mod tests {
    use super::{decode_bars_dir_name, decode_spec_token, encode_bars_dir_name, encode_spec_token};
    use senken_series::{Anchor, BarSpec, BarUnit, Origin};

    #[test]
    fn utc_anchor_adds_no_suffix_for_a_day_spec() {
        assert_eq!(
            encode_spec_token(BarSpec::new(1, BarUnit::Day), Anchor::UTC),
            "1d"
        );
    }

    /// `Anchor`'s own sign convention (aggregate.rs): positive means the
    /// bucket boundary falls *later* than UTC midnight; negative means
    /// *earlier*. A venue at UTC+H rolls its day over H hours *before*
    /// UTC midnight arrives, so its `Anchor` is `-H` hours — this helper
    /// keeps that inversion in one place for the tests below.
    fn anchor_for_venue_utc_offset_hours(h: i64) -> Anchor {
        Anchor::from_offset_nanos(-h * 3_600_000_000_000)
    }

    #[test]
    fn okxs_utc_plus_8_day_round_trips_through_the_path_token() {
        // The exact case: OKX's plain `1D` rolls over at
        // UTC+8 (Hong Kong) midnight, i.e. 16:00 UTC.
        let anchor = anchor_for_venue_utc_offset_hours(8);
        let spec = BarSpec::new(1, BarUnit::Day);
        let token = encode_spec_token(spec, anchor);
        assert_eq!(token, "1d@utc8");
        assert_eq!(decode_spec_token(&token), Some((spec, anchor)));
    }

    #[test]
    fn a_venue_behind_utc_round_trips_with_a_negative_suffix() {
        let anchor = anchor_for_venue_utc_offset_hours(-5);
        let spec = BarSpec::new(1, BarUnit::Week);
        let token = encode_spec_token(spec, anchor);
        assert_eq!(token, "1w@utc-5");
        assert_eq!(decode_spec_token(&token), Some((spec, anchor)));
    }

    #[test]
    fn a_non_hour_anchor_falls_back_to_minute_precision_and_still_round_trips() {
        // No real venue observed uses a non-hour offset; `Anchor` does not
        // forbid one, so the fallback path still needs to round-trip.
        let anchor = Anchor::from_offset_nanos(90 * 60_000_000_000);
        let spec = BarSpec::new(1, BarUnit::Month);
        let token = encode_spec_token(spec, anchor);
        assert_eq!(token, "1mo@utcm-90");
        assert_eq!(decode_spec_token(&token), Some((spec, anchor)));
    }

    #[test]
    fn a_non_minute_anchor_falls_back_to_nanosecond_precision_and_still_round_trips() {
        let anchor = Anchor::from_offset_nanos(-123);
        let spec = BarSpec::new(1, BarUnit::Day);
        let token = encode_spec_token(spec, anchor);
        assert_eq!(token, "1d@utcn123");
        assert_eq!(decode_spec_token(&token), Some((spec, anchor)));
    }

    #[test]
    fn a_non_utc_anchor_below_day_is_never_encoded() {
        // Anchor is documented as meaningless below Day; a caller passing
        // one anyway must not corrupt the token or silently change it.
        let anchor = anchor_for_venue_utc_offset_hours(8);
        assert_eq!(
            encode_spec_token(BarSpec::new(1, BarUnit::Minute), anchor),
            "1m"
        );
    }

    #[test]
    fn bars_dir_name_round_trips_origin_spec_and_anchor_together() {
        let anchor = anchor_for_venue_utc_offset_hours(8);
        let spec = BarSpec::new(1, BarUnit::Day);
        let name = encode_bars_dir_name(Origin::Venue, spec, anchor);
        assert_eq!(name, "venue-1d@utc8");
        assert_eq!(
            decode_bars_dir_name(&name),
            Some((Origin::Venue, spec, anchor))
        );
    }

    #[test]
    fn two_anchors_for_the_same_nominal_spec_produce_different_directory_names() {
        // The whole point of F3: these must not collide.
        let utc = encode_bars_dir_name(Origin::Venue, BarSpec::new(1, BarUnit::Day), Anchor::UTC);
        let utc8 = encode_bars_dir_name(
            Origin::Venue,
            BarSpec::new(1, BarUnit::Day),
            anchor_for_venue_utc_offset_hours(8),
        );
        assert_ne!(utc, utc8);
    }

    #[test]
    fn decode_rejects_an_unknown_anchor_prefix() {
        assert_eq!(decode_spec_token("1d@bogus"), None);
    }

    #[test]
    fn decode_rejects_a_malformed_spec() {
        assert_eq!(decode_bars_dir_name("venue-"), None);
        assert_eq!(decode_bars_dir_name("not-a-known-origin-1d"), None);
    }
}
