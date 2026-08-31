//! [`UnixNanos`] — the one time type used throughout Senken.

use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};

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

#[cfg(test)]
mod tests {
    use super::UnixNanos;
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
}
