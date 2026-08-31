//! Write-side assertions: reject a bad batch of bars outright,
//! never merely warn about it. Every check here operates on plain
//! `senken-series` types — no Arrow — because these are logical
//! invariants about what a bar *means*, not about how it is encoded on
//! disk.
//!
//! A file that passed these checks is safe to binary-search and
//! stream-merge: timestamps are strictly increasing, every bar sits where
//! its spec (and, for Day and above, its declared anchor) says it must,
//! and no bar falls outside the range the file claims to cover.

use senken_core::TimeRange;
use senken_series::{Anchor, BarSpec, bucket_start};

use crate::error::WriteAssertionError;
use crate::spec_token::anchor_applies_to;

/// Validates `bars` against every M5.3 rule for one `(spec, anchor, range)`
/// file. `bars` must already be in the order they will be written — this
/// does not sort them, since silently reordering input would hide the
/// caller's bug rather than reject it.
///
/// # Errors
/// The first [`WriteAssertionError`] found, checked bar by bar in order so
/// the error always names the earliest offending bar.
pub(crate) fn assert_bars_valid(
    bars: &[senken_series::Bar],
    spec: BarSpec,
    anchor: Anchor,
    range: TimeRange,
) -> Result<(), WriteAssertionError> {
    if bars.is_empty() {
        return Err(WriteAssertionError::EmptyBatch);
    }

    let mut previous_ts_open = None;
    for bar in bars {
        if let Some(previous) = previous_ts_open
            && bar.ts_open <= previous
        {
            return Err(WriteAssertionError::NotStrictlyIncreasing {
                previous,
                next: bar.ts_open,
            });
        }
        previous_ts_open = Some(bar.ts_open);

        if bucket_start(bar.ts_open, spec, anchor) != bar.ts_open {
            return Err(if anchor_applies_to(spec.unit) && anchor != Anchor::UTC {
                WriteAssertionError::AnchorMismatch {
                    ts_open: bar.ts_open,
                    spec,
                    offset_nanos: anchor.offset_nanos(),
                }
            } else {
                WriteAssertionError::Misaligned {
                    ts_open: bar.ts_open,
                    spec,
                }
            });
        }

        if !range.contains(bar.ts_open) {
            return Err(WriteAssertionError::OutOfDeclaredRange {
                ts_open: bar.ts_open,
                range_start: range.start(),
                range_end: range.end(),
            });
        }

        if bar.high < bar.low {
            return Err(WriteAssertionError::HighBelowLow {
                ts_open: bar.ts_open,
                high: bar.high,
                low: bar.low,
            });
        }
        if bar.open < bar.low || bar.open > bar.high {
            return Err(WriteAssertionError::OutsideLowHigh {
                ts_open: bar.ts_open,
                field: "open",
                value: bar.open,
                low: bar.low,
                high: bar.high,
            });
        }
        if bar.close < bar.low || bar.close > bar.high {
            return Err(WriteAssertionError::OutsideLowHigh {
                ts_open: bar.ts_open,
                field: "close",
                value: bar.close,
                low: bar.low,
                high: bar.high,
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::assert_bars_valid;
    use crate::error::WriteAssertionError;
    use senken_core::{TimeRange, UnixNanos};
    use senken_series::{Anchor, Bar, BarSpec, BarUnit};

    fn bar(ts_open_secs: i64, open: i64, high: i64, low: i64, close: i64) -> Bar {
        Bar {
            ts_open: UnixNanos::from_secs(ts_open_secs).unwrap(),
            open,
            high,
            low,
            close,
            volume: 1,
            quote_volume: None,
            trade_count: None,
            taker_buy_volume: None,
        }
    }

    fn minute_spec() -> BarSpec {
        BarSpec::new(1, BarUnit::Minute)
    }

    fn wide_range() -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(0).unwrap(),
            UnixNanos::from_secs(86_400).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_well_formed_batch_is_accepted() {
        let bars = [bar(0, 1, 2, 1, 2), bar(60, 2, 3, 1, 2)];
        assert_bars_valid(&bars, minute_spec(), Anchor::UTC, wide_range()).unwrap();
    }

    #[test]
    fn an_empty_batch_is_rejected() {
        assert_eq!(
            assert_bars_valid(&[], minute_spec(), Anchor::UTC, wide_range()),
            Err(WriteAssertionError::EmptyBatch)
        );
    }

    #[test]
    fn a_misaligned_ts_open_is_rejected() {
        let bars = [bar(30, 1, 2, 1, 2)]; // 30s into a minute, not on a boundary
        assert!(matches!(
            assert_bars_valid(&bars, minute_spec(), Anchor::UTC, wide_range()),
            Err(WriteAssertionError::Misaligned { .. })
        ));
    }

    #[test]
    fn a_day_bar_disagreeing_with_its_declared_anchor_is_rejected() {
        let day = BarSpec::new(1, BarUnit::Day);
        let anchor = Anchor::from_offset_nanos(8 * 3_600_000_000_000);
        // Aligned to UTC midnight, but the series declares a +8h anchor
        // (whose boundary falls at 08:00 UTC, not 00:00 UTC).
        let bars = [bar(0, 1, 2, 1, 2)];
        let range = TimeRange::new(
            UnixNanos::from_secs(-100_000).unwrap(),
            UnixNanos::from_secs(1_000_000).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            assert_bars_valid(&bars, day, anchor, range),
            Err(WriteAssertionError::AnchorMismatch { .. })
        ));
    }

    #[test]
    fn a_day_bar_matching_its_declared_anchor_is_accepted() {
        let day = BarSpec::new(1, BarUnit::Day);
        let anchor = Anchor::from_offset_nanos(8 * 3_600_000_000_000);
        // A boundary at offset +8h falls at 08:00 UTC (`Anchor`'s own
        // sign convention: positive is *later* than UTC midnight).
        let bars = [bar(8 * 3600, 1, 2, 1, 2)];
        let range = TimeRange::new(
            UnixNanos::from_secs(-100_000).unwrap(),
            UnixNanos::from_secs(1_000_000).unwrap(),
        )
        .unwrap();
        assert_bars_valid(&bars, day, anchor, range).unwrap();
    }

    #[test]
    fn duplicated_timestamps_are_rejected() {
        let bars = [bar(0, 1, 2, 1, 2), bar(0, 1, 2, 1, 2)];
        assert!(matches!(
            assert_bars_valid(&bars, minute_spec(), Anchor::UTC, wide_range()),
            Err(WriteAssertionError::NotStrictlyIncreasing { .. })
        ));
    }

    #[test]
    fn out_of_order_timestamps_are_rejected() {
        let bars = [bar(60, 1, 2, 1, 2), bar(0, 1, 2, 1, 2)];
        assert!(matches!(
            assert_bars_valid(&bars, minute_spec(), Anchor::UTC, wide_range()),
            Err(WriteAssertionError::NotStrictlyIncreasing { .. })
        ));
    }

    #[test]
    fn a_bar_outside_the_declared_range_is_rejected() {
        let narrow = TimeRange::new(
            UnixNanos::from_secs(0).unwrap(),
            UnixNanos::from_secs(60).unwrap(),
        )
        .unwrap();
        let bars = [bar(0, 1, 2, 1, 2), bar(60, 1, 2, 1, 2)]; // second bar is at the exclusive end
        assert!(matches!(
            assert_bars_valid(&bars, minute_spec(), Anchor::UTC, narrow),
            Err(WriteAssertionError::OutOfDeclaredRange { .. })
        ));
    }

    #[test]
    fn high_below_low_is_rejected() {
        let bars = [bar(0, 1, 1, 2, 1)]; // high (1) < low (2)
        assert!(matches!(
            assert_bars_valid(&bars, minute_spec(), Anchor::UTC, wide_range()),
            Err(WriteAssertionError::HighBelowLow { .. })
        ));
    }

    #[test]
    fn open_outside_low_high_is_rejected() {
        let bars = [bar(0, 10, 5, 1, 3)]; // open (10) > high (5)
        assert!(matches!(
            assert_bars_valid(&bars, minute_spec(), Anchor::UTC, wide_range()),
            Err(WriteAssertionError::OutsideLowHigh { field: "open", .. })
        ));
    }

    #[test]
    fn close_outside_low_high_is_rejected() {
        let bars = [bar(0, 2, 5, 1, 10)]; // close (10) > high (5)
        assert!(matches!(
            assert_bars_valid(&bars, minute_spec(), Anchor::UTC, wide_range()),
            Err(WriteAssertionError::OutsideLowHigh { field: "close", .. })
        ));
    }
}
