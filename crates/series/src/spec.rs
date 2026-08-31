//! [`BarSpec`], [`BarUnit`] and [`Origin`] — what a series of bars *is*
//! .

use std::fmt;
use std::num::NonZeroU32;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A bar timeframe: `step` repetitions of `unit`, e.g. 15 [`BarUnit::Minute`]
/// for a 15-minute bar.
///
/// Deliberately an **open struct, not a closed enum**. A closed
/// `enum Timeframe { M1, M5, H1 }` cannot express M3 or H2, and forecloses
/// the volume/tick/Renko aggregation units this shape gives away for free
/// later — those units are out of scope for this plan
/// (Part D.4) but must not require a schema change to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BarSpec {
    /// How many `unit`s make up one bar. Always at least one — a zero-length
    /// bar is meaningless, so this is unrepresentable rather than merely
    /// disallowed by convention.
    pub step: NonZeroU32,
    /// The unit `step` counts in.
    pub unit: BarUnit,
}

impl BarSpec {
    /// A convenience constructor taking a plain `step`.
    ///
    /// # Panics
    /// If `step` is zero. Prefer constructing the struct literal directly
    /// with a [`NonZeroU32`] when `step` is not already known to be
    /// non-zero.
    #[must_use]
    pub fn new(step: u32, unit: BarUnit) -> Self {
        Self {
            step: NonZeroU32::new(step).expect("BarSpec::new requires a non-zero step"),
            unit,
        }
    }

    /// This spec's fixed length in nanoseconds, or `None` for
    /// [`BarUnit::Month`], whose length is not fixed — a calendar month is
    /// 28 to 31 days, so "how many nanoseconds is one month" has no single
    /// correct answer independent of *which* month.
    ///
    /// Every other unit is calendar-independent: a second, minute, hour,
    /// day and week always have the same length, so their duration is a
    /// pure function of `step` alone.
    #[must_use]
    pub fn duration_nanos(self) -> Option<i64> {
        let unit_nanos: i64 = match self.unit {
            BarUnit::Second => 1_000_000_000,
            BarUnit::Minute => 60_000_000_000,
            BarUnit::Hour => 3_600_000_000_000,
            BarUnit::Day => 86_400_000_000_000,
            BarUnit::Week => 7 * 86_400_000_000_000,
            BarUnit::Month => return None,
        };
        Some(unit_nanos * i64::from(self.step.get()))
    }
}

impl fmt::Display for BarSpec {
    /// The path token used in file and directory names: a
    /// plain integer step followed by a single-letter unit suffix, e.g.
    /// `1m`, `15m`, `1h`, `1d`.
    ///
    /// [`BarUnit::Month`] uses `mo`, not `M` or `m`, deliberately: this
    /// project already upper-cases symbols for exactly this reason (path
    /// key) — several of this project's target filesystems
    /// (Windows, and macOS's default APFS) are case-insensitive, so a
    /// bare-letter-case distinction between "minute" and "month" would be
    /// two different bars sharing one directory on those platforms. `mo` is
    /// unambiguous regardless of case folding.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.step, self.unit.suffix())
    }
}

impl FromStr for BarSpec {
    type Err = ParseBarSpecError;

    /// Reverses [`Display`](fmt::Display): a run of ASCII digits (the step)
    /// followed immediately by one of the suffixes named there, and nothing
    /// else.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let digits_len = s.bytes().take_while(u8::is_ascii_digit).count();
        let (digits, suffix) = s.split_at(digits_len);
        if digits.is_empty() {
            return Err(ParseBarSpecError::MissingStep(s.to_owned()));
        }
        let step: u32 = digits
            .parse()
            .map_err(|_| ParseBarSpecError::StepOutOfRange(s.to_owned()))?;
        let step =
            NonZeroU32::new(step).ok_or_else(|| ParseBarSpecError::ZeroStep(s.to_owned()))?;
        let unit = BarUnit::from_suffix(suffix)
            .ok_or_else(|| ParseBarSpecError::UnknownUnit(suffix.to_owned(), s.to_owned()))?;
        Ok(Self { step, unit })
    }
}

/// Why [`BarSpec::from_str`] could not parse a path token.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParseBarSpecError {
    /// The string had no leading digits at all.
    #[error("{0:?} has no numeric step")]
    MissingStep(String),
    /// The leading digits parsed as zero, or overflowed `u32`.
    #[error("{0:?}'s step must be a non-zero u32")]
    ZeroStep(String),
    /// The leading digits overflowed `u32`.
    #[error("{0:?}'s step does not fit a u32")]
    StepOutOfRange(String),
    /// The suffix after the digits did not match any [`BarUnit`].
    #[error("{0:?} is not a known bar unit (in {1:?})")]
    UnknownUnit(String, String),
}

/// The unit [`BarSpec::step`] counts in.
///
/// `#[non_exhaustive]`: this project only builds time bars (Part D.4), but
/// the whole point of [`BarSpec`] being an open struct is that
/// volume/tick/Renko units can be added here later without a schema
/// change — callers outside this crate must already be written to handle
/// "a unit I don't recognise yet".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BarUnit {
    /// One second.
    Second,
    /// One minute.
    Minute,
    /// One hour.
    Hour,
    /// One calendar day, midnight to midnight at some [`Anchor`](crate::Anchor).
    Day,
    /// Seven days, Monday to Monday at some [`Anchor`](crate::Anchor) — a
    /// convention with no basis in the plan or design record, chosen here
    /// because it matches how every mainstream charting platform
    /// (`TradingView`, MT5) aligns a weekly bar; revisit if that turns out to
    /// be wrong for a 24/7 crypto venue.
    Week,
    /// One calendar month, the 1st to the 1st. Not a fixed duration — see
    /// [`BarSpec::duration_nanos`].
    Month,
}

impl BarUnit {
    /// The [`Display`](fmt::Display) suffix for this unit — see
    /// [`BarSpec`]'s `Display` impl for why [`Self::Month`] gets a
    /// two-letter suffix.
    #[must_use]
    fn suffix(self) -> &'static str {
        match self {
            Self::Second => "s",
            Self::Minute => "m",
            Self::Hour => "h",
            Self::Day => "d",
            Self::Week => "w",
            Self::Month => "mo",
        }
    }

    /// Reverses [`Self::suffix`]. Matches longest-first (`mo` before any
    /// single-letter suffix) so a caller cannot accidentally construct an
    /// ambiguous suffix table by adding a unit later.
    #[must_use]
    fn from_suffix(s: &str) -> Option<Self> {
        match s {
            "s" => Some(Self::Second),
            "m" => Some(Self::Minute),
            "h" => Some(Self::Hour),
            "d" => Some(Self::Day),
            "w" => Some(Self::Week),
            "mo" => Some(Self::Month),
            _ => None,
        }
    }
}

/// Whether a bar came from the venue, or was aggregated locally
/// .
///
/// Part of [`SeriesKey`](crate::SeriesKey), not a side annotation: a venue
/// M1 and a locally-derived M1 for the same symbol are different data with
/// different biases, and merging them into one series produces a series
/// that is wrong in a way no test will catch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Origin {
    /// The venue handed us this bar directly.
    Venue,
    /// This bar was aggregated here, from a finer stored spec.
    Derived,
}

impl fmt::Display for Origin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Venue => "venue",
            Self::Derived => "derived",
        })
    }
}

impl FromStr for Origin {
    type Err = ParseOriginError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "venue" => Ok(Self::Venue),
            "derived" => Ok(Self::Derived),
            other => Err(ParseOriginError(other.to_owned())),
        }
    }
}

/// [`Origin::from_str`] only accepts exactly `"venue"` or `"derived"`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not \"venue\" or \"derived\"")]
pub struct ParseOriginError(String);

#[cfg(test)]
mod tests {
    use super::{BarSpec, BarUnit, Origin, ParseBarSpecError};

    fn spec(step: u32, unit: BarUnit) -> BarSpec {
        BarSpec::new(step, unit)
    }

    #[test]
    fn bar_spec_round_trips_through_display_and_from_str() {
        let cases = [
            spec(1, BarUnit::Second),
            spec(1, BarUnit::Minute),
            spec(15, BarUnit::Minute),
            spec(1, BarUnit::Hour),
            spec(4, BarUnit::Hour),
            spec(1, BarUnit::Day),
            spec(1, BarUnit::Week),
            spec(3, BarUnit::Month),
        ];
        for spec in cases {
            let rendered = spec.to_string();
            assert_eq!(rendered.parse::<BarSpec>().unwrap(), spec, "{rendered}");
        }
    }

    #[test]
    fn bar_spec_display_matches_the_documented_path_tokens() {
        assert_eq!(spec(1, BarUnit::Minute).to_string(), "1m");
        assert_eq!(spec(15, BarUnit::Minute).to_string(), "15m");
        assert_eq!(spec(1, BarUnit::Hour).to_string(), "1h");
        assert_eq!(spec(1, BarUnit::Day).to_string(), "1d");
    }

    #[test]
    fn minute_and_month_do_not_collide_case_insensitively() {
        // The whole reason `Month` gets a two-letter suffix: on a
        // case-insensitive filesystem, a single-letter `M`/`m` pair for
        // minute and month would be the same directory entry.
        let minute = spec(1, BarUnit::Minute).to_string();
        let month = spec(1, BarUnit::Month).to_string();
        assert_ne!(minute.to_lowercase(), month.to_lowercase());
    }

    #[test]
    fn from_str_rejects_a_missing_step() {
        assert_eq!(
            "m".parse::<BarSpec>(),
            Err(ParseBarSpecError::MissingStep("m".to_owned()))
        );
        assert_eq!(
            "".parse::<BarSpec>(),
            Err(ParseBarSpecError::MissingStep(String::new()))
        );
    }

    #[test]
    fn from_str_rejects_a_zero_step() {
        assert_eq!(
            "0m".parse::<BarSpec>(),
            Err(ParseBarSpecError::ZeroStep("0m".to_owned()))
        );
    }

    #[test]
    fn from_str_rejects_an_unknown_unit() {
        assert_eq!(
            "15x".parse::<BarSpec>(),
            Err(ParseBarSpecError::UnknownUnit(
                "x".to_owned(),
                "15x".to_owned()
            ))
        );
    }

    #[test]
    fn from_str_rejects_trailing_garbage_after_a_known_unit() {
        // `15mx` must not silently parse as a 15-minute spec with the
        // trailing `x` ignored.
        assert!("15mx".parse::<BarSpec>().is_err());
    }

    #[test]
    fn origin_round_trips_through_display_and_from_str() {
        for origin in [Origin::Venue, Origin::Derived] {
            assert_eq!(origin.to_string().parse::<Origin>().unwrap(), origin);
        }
    }

    #[test]
    fn origin_display_matches_the_documented_tokens() {
        assert_eq!(Origin::Venue.to_string(), "venue");
        assert_eq!(Origin::Derived.to_string(), "derived");
    }

    #[test]
    fn origin_from_str_rejects_anything_else() {
        assert!(
            "Venue".parse::<Origin>().is_err(),
            "case must match exactly"
        );
        assert!("".parse::<Origin>().is_err());
    }

    #[test]
    fn duration_nanos_is_none_only_for_month() {
        assert_eq!(
            spec(1, BarUnit::Second).duration_nanos(),
            Some(1_000_000_000)
        );
        assert_eq!(
            spec(1, BarUnit::Minute).duration_nanos(),
            Some(60_000_000_000)
        );
        assert_eq!(
            spec(1, BarUnit::Week).duration_nanos(),
            Some(7 * 86_400_000_000_000)
        );
        assert_eq!(spec(1, BarUnit::Month).duration_nanos(), None);
    }
}
