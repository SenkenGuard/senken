//! [`UnixNanos`] — the one time type used throughout Senken — plus
//! [`CivilDateTime`] and [`instant_from_civil`], the only door from a
//! wall-clock date and time into one.
//!
//! # Why the time zone database is bundled
//!
//! The host machine's own copy of the IANA Time Zone Database differs by
//! machine and by how recently its OS was updated. A backtest that resolves
//! a civil datetime to an instant must produce the same instant wherever it
//! runs, so this crate links a fixed copy of the database into the binary
//! (via `jiff`'s `tzdb-bundle-always` feature) instead of asking the host.
//! The host machine's own default time zone is never read at all — nothing
//! in this module calls `jiff::tz::TimeZone::system`, and the feature that
//! backs it is left disabled in this crate's `jiff` dependency.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::zone::IanaZone;

/// Nanoseconds since `1970-01-01T00:00:00Z`, UTC, always.
///
/// This is deliberately the *only* time type in the model: no seconds, no
/// milliseconds, no local time. Mixed units are this project's most
/// expensive recurring bug — Gate reports delivery expiry in **seconds**
/// while every neighbouring field is milliseconds, and that shipped and had
/// to be found by hand. A raw `i64` cannot be rejected at a call site
/// because it carries no unit; a newtype can. There is deliberately no
/// `From<i64>` impl for exactly this reason — a conversion must always name
/// its unit ([`from_millis`](Self::from_millis), [`from_secs`](Self::from_secs)
/// or, for a value already in nanoseconds, [`from_nanos`](Self::from_nanos)).
///
/// Nanosecond precision exists for ticks, which venues timestamp at µs/ns
/// resolution and where several trades routinely share a millisecond. Bars
/// do not need that precision but still use the same type, so nothing in
/// the model ever has to convert between two time representations.
///
/// Serialises as a plain integer number of nanoseconds. [`Display`](fmt::Display)
/// renders RFC 3339 UTC, for logs and error messages.
///
/// Range: an `i64` count of nanoseconds covers roughly the years 1678–2262,
/// which is adequate for this project's purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct UnixNanos(i64);

impl UnixNanos {
    /// `1970-01-01T00:00:00Z`.
    pub const EPOCH: Self = Self(0);

    /// Wraps a value already known to be nanoseconds since the epoch.
    ///
    /// Named (rather than a `From<i64>` impl) so every call site states
    /// that its input actually is nanoseconds.
    #[must_use]
    pub const fn from_nanos(n: i64) -> Self {
        Self(n)
    }

    /// Converts milliseconds since the epoch, checked against `i64`
    /// overflow.
    ///
    /// Checked because silent overflow here is silent corruption of a
    /// timestamp — the same class of bug the newtype itself exists to
    /// prevent.
    #[must_use]
    pub const fn from_millis(ms: i64) -> Option<Self> {
        match ms.checked_mul(1_000_000) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    /// Converts seconds since the epoch, checked against `i64` overflow.
    #[must_use]
    pub const fn from_secs(s: i64) -> Option<Self> {
        match s.checked_mul(1_000_000_000) {
            Some(n) => Some(Self(n)),
            None => None,
        }
    }

    /// The raw nanosecond count.
    #[must_use]
    pub const fn as_nanos(self) -> i64 {
        self.0
    }

    /// The value truncated to whole milliseconds.
    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0 / 1_000_000
    }

    /// Adds `d`, returning `None` on `i64` overflow rather than wrapping.
    #[must_use]
    pub fn checked_add(self, d: Duration) -> Option<Self> {
        let nanos = i64::try_from(d.as_nanos()).ok()?;
        self.0.checked_add(nanos).map(Self)
    }

    /// `true` when this instant falls exactly on a multiple of `interval`
    /// since the epoch.
    ///
    /// A zero-length interval is never aligned to, since nothing is a
    /// multiple of zero.
    #[must_use]
    pub fn is_aligned_to(self, interval: Duration) -> bool {
        let Ok(interval_nanos) = i64::try_from(interval.as_nanos()) else {
            return false;
        };
        if interval_nanos == 0 {
            return false;
        }
        self.0 % interval_nanos == 0
    }
}

impl fmt::Display for UnixNanos {
    /// Renders RFC 3339 UTC, e.g. `2026-08-30T12:34:56Z` or, when the
    /// instant carries sub-second precision, `2026-08-30T12:34:56.123456789Z`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Split into a day count and a nanosecond-of-day offset, using
        // Euclidean division so this is correct for instants before the
        // epoch too (a negative `self.0` still yields a non-negative
        // remainder).
        let secs = self.0.div_euclid(1_000_000_000);
        let nanos = self.0.rem_euclid(1_000_000_000);
        let days = secs.div_euclid(86_400);
        let sec_of_day = secs.rem_euclid(86_400);

        let (year, month, day) = civil_from_days(days);
        let hour = sec_of_day / 3600;
        let minute = (sec_of_day % 3600) / 60;
        let second = sec_of_day % 60;

        if nanos == 0 {
            write!(
                f,
                "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
            )
        } else {
            write!(
                f,
                "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{nanos:09}Z"
            )
        }
    }
}

/// Converts a day count since the epoch (may be negative) into a proleptic
/// Gregorian `(year, month, day)`.
///
/// Adapted from Howard Hinnant's `civil_from_days`
/// (<http://howardhinnant.github.io/date_algorithms.html>), a public-domain
/// integer algorithm — kept entirely in `i64` and free of `as` casts so it
/// carries no truncation or sign-loss risk.
///
/// Exported (not private to this module) because it is calendar arithmetic
/// in its own right, independent of [`UnixNanos`]'s `Display` impl above —
/// `senken-series`'s day/week/month bucket boundaries need exactly the same
/// conversion. M4 shipped with a second, independently-written copy of this
/// function there; that duplication is the defect this export fixes (plan
/// 001): two copies of calendar math drift silently, and the
/// visible symptom would be a bar's *displayed* timestamp disagreeing with
/// the bucket it was *placed* in.
#[must_use]
pub fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

/// Inverse of [`civil_from_days`]: a proleptic Gregorian `(year, month,
/// day)` into a day count since the epoch (may be negative). Same source
/// and public-domain status; kept entirely in `i64` for the same reason.
#[must_use]
pub fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe - 719_468
}

/// A wall-clock date and time with no attached zone, e.g. `2026-09-01
/// 09:00:00`.
///
/// On its own this value is not meaningful: the same digits name a
/// different instant in every zone they might be read in. This type exists
/// only to be paired with an [`IanaZone`] — nothing in this crate can reach
/// a [`UnixNanos`] from a bare `CivilDateTime`; the only door is
/// [`instant_from_civil`], which takes a zone as a required argument, not a
/// default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CivilDateTime(jiff::civil::DateTime);

impl CivilDateTime {
    /// Validates a calendar date and a time of day, e.g. rejects `month:
    /// 13` or `day: 30` in February.
    ///
    /// # Errors
    ///
    /// Returns [`CivilDateTimeError`] when the fields do not form a valid
    /// calendar date and time.
    pub fn new(
        year: i16,
        month: i8,
        day: i8,
        hour: i8,
        minute: i8,
        second: i8,
        nanosecond: i32,
    ) -> Result<Self, CivilDateTimeError> {
        jiff::civil::DateTime::new(year, month, day, hour, minute, second, nanosecond)
            .map(Self)
            .map_err(|e| CivilDateTimeError(e.to_string()))
    }
}

impl fmt::Display for CivilDateTime {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl std::str::FromStr for CivilDateTime {
    type Err = CivilDateTimeError;

    /// Parses an ISO 8601 civil datetime, e.g. `"2026-09-01T09:00:00"`. No
    /// offset and no zone are accepted — a string carrying either is exactly
    /// the "bare civil datetime that might mean anything" shape this type
    /// exists to rule out at the type level; pass the zone separately to
    /// [`instant_from_civil`].
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<jiff::civil::DateTime>()
            .map(Self)
            .map_err(|e| CivilDateTimeError(e.to_string()))
    }
}

/// A string, or a set of fields, that does not form a valid calendar
/// datetime.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid civil datetime: {0}")]
pub struct CivilDateTimeError(String);

/// An error converting a civil datetime to an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum TimeError {
    /// The resulting instant does not fit in [`UnixNanos`]'s representable
    /// range (roughly the years 1678–2262).
    #[error("civil datetime is out of range for an instant")]
    OutOfRange,
}

/// Converts a civil (wall-clock) datetime to an instant, read in `zone`.
///
/// This is the **only** function in this crate that turns a civil datetime
/// into a [`UnixNanos`]. The zone is a required parameter — there is no
/// overload or default that omits it, so a caller cannot reach an instant
/// without naming one. Deleting the `zone` parameter and calling this with
/// only a `civil` argument is a compile error (`E0061`, wrong number of
/// arguments), not a runtime one.
///
/// # DST ambiguity
///
/// A "spring forward" transition skips an hour of wall-clock time — the
/// given civil datetime may never have occurred in `zone`. A "fall back"
/// transition repeats one — it may have occurred twice, at two different UTC
/// offsets. Both are resolved the same way every time, here, rather than
/// left for each caller to guess: a skipped hour resolves to the offset
/// *after* the transition, and a repeated hour resolves to its *first*
/// occurrence (the offset in force *before* the transition). This is the
/// "compatible" strategy from RFC 5545 (iCalendar), chosen because it is the
/// rule most existing calendar tooling already uses for the same ambiguity.
///
/// # Errors
///
/// Returns [`TimeError::OutOfRange`] when the resulting instant falls
/// outside the range [`UnixNanos`] can represent.
pub fn instant_from_civil(civil: CivilDateTime, zone: &IanaZone) -> Result<UnixNanos, TimeError> {
    let zoned = zone
        .to_jiff()
        .to_ambiguous_zoned(civil.0)
        .compatible()
        .map_err(|_| TimeError::OutOfRange)?;
    let nanos = zoned.timestamp().as_nanosecond();
    i64::try_from(nanos)
        .map(UnixNanos::from_nanos)
        .map_err(|_| TimeError::OutOfRange)
}

#[cfg(test)]
mod tests {
    use super::{CivilDateTime, UnixNanos, days_from_civil, instant_from_civil};
    use crate::zone::IanaZone;
    use std::time::Duration;

    #[test]
    fn from_millis_checks_overflow_instead_of_wrapping() {
        assert_eq!(
            UnixNanos::from_millis(1_000),
            Some(UnixNanos::from_nanos(1_000_000_000))
        );
        assert_eq!(UnixNanos::from_millis(i64::MAX), None);
        assert_eq!(UnixNanos::from_millis(i64::MIN), None);
    }

    #[test]
    fn from_secs_checks_overflow_instead_of_wrapping() {
        assert_eq!(
            UnixNanos::from_secs(1),
            Some(UnixNanos::from_nanos(1_000_000_000))
        );
        assert_eq!(UnixNanos::from_secs(i64::MAX), None);
        assert_eq!(UnixNanos::from_secs(i64::MIN), None);
    }

    #[test]
    fn as_millis_truncates_towards_zero() {
        assert_eq!(UnixNanos::from_nanos(1_999_999).as_millis(), 1);
        assert_eq!(UnixNanos::from_nanos(-1_999_999).as_millis(), -1);
    }

    #[test]
    fn checked_add_reports_overflow() {
        let t = UnixNanos::from_nanos(i64::MAX - 5);
        assert_eq!(
            t.checked_add(Duration::from_nanos(5)),
            Some(UnixNanos::from_nanos(i64::MAX))
        );
        assert_eq!(t.checked_add(Duration::from_nanos(6)), None);
    }

    #[test]
    fn alignment_checks_multiples_of_the_interval() {
        let minute = Duration::from_mins(1);
        assert!(UnixNanos::from_secs(120).unwrap().is_aligned_to(minute));
        assert!(!UnixNanos::from_secs(90).unwrap().is_aligned_to(minute));
        assert!(UnixNanos::EPOCH.is_aligned_to(minute));
    }

    #[test]
    fn alignment_to_a_zero_interval_is_never_true() {
        assert!(!UnixNanos::EPOCH.is_aligned_to(Duration::ZERO));
    }

    #[test]
    fn display_renders_rfc3339_utc() {
        assert_eq!(UnixNanos::EPOCH.to_string(), "1970-01-01T00:00:00Z");
        // 2026-08-30T00:00:00Z, the date this plan was captured.
        assert_eq!(
            UnixNanos::from_secs(1_788_048_000).unwrap().to_string(),
            "2026-08-30T00:00:00Z"
        );
    }

    #[test]
    fn display_includes_nanoseconds_when_present() {
        let t = UnixNanos::from_nanos(1_788_048_000_123_456_789);
        assert_eq!(t.to_string(), "2026-08-30T00:00:00.123456789Z");
    }

    #[test]
    fn display_handles_instants_before_the_epoch() {
        // One second and one nanosecond before the epoch.
        let t = UnixNanos::from_nanos(-1_000_000_001);
        assert_eq!(t.to_string(), "1969-12-31T23:59:58.999999999Z");
    }

    #[test]
    fn civil_from_days_and_days_from_civil_round_trip_across_a_wide_range() {
        // Exercises leap years, non-leap century years, and both sides of
        // the epoch — the corners that a Gregorian day-count algorithm is
        // most likely to get wrong.
        for day in [
            -719_162, // proleptic year 0
            -1,       // the day before the epoch
            0,        // the epoch itself
            30_557,   // 2054-01-01, arbitrary far future
            11_017,   // 2000-02-29, a leap day in a leap century year
            -54_465,  // 1900-02-28, the day before a non-leap century "leap" day
        ] {
            let (year, month, day_of_month) = super::civil_from_days(day);
            assert_eq!(super::days_from_civil(year, month, day_of_month), day);
        }
    }

    /// `2026-09-01T09:00:00` read in `Asia/Jakarta` (UTC+7, no DST) is
    /// `2026-09-01T02:00:00Z` — the one case in this file that a bug tied to
    /// the *test process's own* zone could hide behind, since it would still
    /// pass if the conversion secretly used the host's local time instead of
    /// `zone`. The zone-crossing tests below are the ones that actually
    /// prove that did not happen.
    #[test]
    fn civil_datetime_in_jakarta_converts_to_the_expected_instant() {
        let civil = CivilDateTime::new(2026, 9, 1, 9, 0, 0, 0).unwrap();
        let zone = IanaZone::new("Asia/Jakarta").unwrap();
        let days = days_from_civil(2026, 9, 1);
        let expected = UnixNanos::from_secs(days * 86_400 + 2 * 3600).unwrap();
        assert_eq!(instant_from_civil(civil, &zone).unwrap(), expected);
    }

    /// The same wall-clock date and time, read in two different zones, must
    /// resolve to two different instants — proof that `zone` is actually
    /// consulted rather than some fixed or host-derived offset.
    #[test]
    fn the_same_civil_datetime_means_different_instants_in_different_zones() {
        let civil = CivilDateTime::new(2026, 9, 1, 9, 0, 0, 0).unwrap();
        let jakarta = instant_from_civil(civil, &IanaZone::new("Asia/Jakarta").unwrap()).unwrap();
        let london = instant_from_civil(civil, &IanaZone::new("Europe/London").unwrap()).unwrap();
        let new_york =
            instant_from_civil(civil, &IanaZone::new("America/New_York").unwrap()).unwrap();
        assert_ne!(jakarta, london);
        assert_ne!(london, new_york);
        assert_ne!(jakarta, new_york);
    }

    /// `America/New_York` springs forward at 2024-03-10 02:00 local, skipping
    /// straight to 03:00 — `02:30` never happened that day. The documented
    /// policy resolves a skipped hour to the offset *after* the transition
    /// (EDT, UTC-4), so `2024-03-10T02:30:00` local becomes
    /// `2024-03-10T03:30:00-04:00`, i.e. `2024-03-10T07:30:00Z`.
    #[test]
    fn spring_forward_gap_resolves_to_the_offset_after_the_transition() {
        let civil = CivilDateTime::new(2024, 3, 10, 2, 30, 0, 0).unwrap();
        let zone = IanaZone::new("America/New_York").unwrap();
        let days = days_from_civil(2024, 3, 10);
        let expected = UnixNanos::from_secs(days * 86_400 + 7 * 3600 + 30 * 60).unwrap();
        assert_eq!(instant_from_civil(civil, &zone).unwrap(), expected);
    }

    /// `America/New_York` falls back at 2024-11-03 02:00 EDT, which becomes
    /// 01:00 EST — `01:30` local happens twice that day, once at each
    /// offset. The documented policy resolves a repeated hour to its first
    /// occurrence, the pre-transition offset (EDT, UTC-4), so
    /// `2024-11-03T01:30:00` local becomes `2024-11-03T01:30:00-04:00`, i.e.
    /// `2024-11-03T05:30:00Z` — not the `06:30:00Z` the post-transition
    /// (EST, UTC-5) offset would give.
    #[test]
    fn fall_back_fold_resolves_to_the_first_occurrence() {
        let civil = CivilDateTime::new(2024, 11, 3, 1, 30, 0, 0).unwrap();
        let zone = IanaZone::new("America/New_York").unwrap();
        let days = days_from_civil(2024, 11, 3);
        let expected = UnixNanos::from_secs(days * 86_400 + 5 * 3600 + 30 * 60).unwrap();
        assert_eq!(instant_from_civil(civil, &zone).unwrap(), expected);
    }

    #[test]
    fn civil_datetime_rejects_an_invalid_calendar_date() {
        // 2023 is not a leap year: February has 28 days.
        assert!(CivilDateTime::new(2023, 2, 29, 0, 0, 0, 0).is_err());
    }

    #[test]
    fn civil_datetime_parses_and_displays_iso_8601() {
        let civil: CivilDateTime = "2026-09-01T09:00:00".parse().unwrap();
        assert_eq!(civil.to_string(), "2026-09-01T09:00:00");
    }
}
