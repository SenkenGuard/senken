//! Filename-encoded coverage ranges.
//!
//! A file's name states the [`TimeRange`] it was fetched for — the whole
//! point being that a directory listing answers "what coverage exists?"
//! with no side table and no file opened. That only works if the encoding
//! is exact (a lossy round-trip would misreport coverage) and sorts the
//! same way lexicographically as chronologically (so pruning by filename,
//! M5.4, can use plain string comparison).
//!
//! No Arrow or Parquet dependency: this is pure string/integer arithmetic,
//! available even with `default-features = false`.

use senken_core::{TimeRange, UnixNanos, civil_from_days, days_from_civil};

const NANOS_PER_DAY: i64 = 86_400_000_000_000;

/// One boundary instant's fixed-width token: `YYYYMMDDTHHMMSSNNNNNNNNN`
/// (24 ASCII bytes — always this exact width, so two tokens compare
/// lexicographically exactly as their instants compare chronologically).
///
/// Nanosecond precision is carried in full, not truncated to the minute
/// shown in the design record's illustrative filenames: `UnixNanos` is a
/// nanosecond type end to end, and a lossy encoding here would
/// break the round-trip this module's own required tests demand for *any*
/// `TimeRange`, not merely ones that happen to be minute-aligned.
fn encode_instant(t: UnixNanos) -> String {
    let total_nanos = t.as_nanos();
    let day_index = total_nanos.div_euclid(NANOS_PER_DAY);
    let nanos_of_day = total_nanos.rem_euclid(NANOS_PER_DAY);

    let (year, month, day) = civil_from_days(day_index);
    let hour = nanos_of_day / 3_600_000_000_000;
    let minute = (nanos_of_day / 60_000_000_000) % 60;
    let second = (nanos_of_day / 1_000_000_000) % 60;
    let nanos = nanos_of_day % 1_000_000_000;

    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}{nanos:09}")
}

/// The exact byte width of one [`encode_instant`] token.
const INSTANT_LEN: usize = 24;

/// Inverse of [`encode_instant`]. Rejects malformed input rather than
/// panicking — a directory can contain files this crate did not write.
fn decode_instant(s: &str) -> Option<UnixNanos> {
    let bytes = s.as_bytes();
    if bytes.len() != INSTANT_LEN || bytes[8] != b'T' {
        return None;
    }
    let digits = |range: std::ops::Range<usize>| s.get(range)?.parse::<i64>().ok();

    let year = digits(0..4)?;
    let month = digits(4..6)?;
    let day = digits(6..8)?;
    let hour = digits(9..11)?;
    let minute = digits(11..13)?;
    let second = digits(13..15)?;
    let nanos = digits(15..24)?;

    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
        || !(0..1_000_000_000).contains(&nanos)
    {
        return None;
    }

    let day_index = days_from_civil(year, month, day);
    let nanos_of_day =
        hour * 3_600_000_000_000 + minute * 60_000_000_000 + second * 1_000_000_000 + nanos;
    let total = day_index
        .checked_mul(NANOS_PER_DAY)?
        .checked_add(nanos_of_day)?;
    Some(UnixNanos::from_nanos(total))
}

/// Encodes `r` as the sortable, round-trippable token used in a bar or
/// trade filename (without the `.parquet` extension), e.g.
/// `20240101T000000000000000_20240201T000000000000000` for the whole of
/// January 2024 UTC.
#[must_use]
pub fn encode_range(r: TimeRange) -> String {
    format!("{}_{}", encode_instant(r.start()), encode_instant(r.end()))
}

/// Inverse of [`encode_range`]. Accepts either the bare token or a full
/// filename ending in `.parquet` (the form a directory listing hands
/// back), so a caller scanning a directory can pass entries straight
/// through. Returns `None` for anything that does not parse as two valid
/// instants forming `end >= start`.
#[must_use]
pub fn decode_range(name: &str) -> Option<TimeRange> {
    let stem = name.strip_suffix(".parquet").unwrap_or(name);
    let (start_s, end_s) = stem.split_once('_')?;
    let start = decode_instant(start_s)?;
    let end = decode_instant(end_s)?;
    TimeRange::new(start, end)
}

#[cfg(test)]
mod tests {
    use super::{decode_range, encode_range};
    use senken_core::{TimeRange, UnixNanos};

    fn range(start_nanos: i64, end_nanos: i64) -> TimeRange {
        TimeRange::new(
            UnixNanos::from_nanos(start_nanos),
            UnixNanos::from_nanos(end_nanos),
        )
        .unwrap()
    }

    #[test]
    fn encode_then_decode_round_trips_a_whole_month() {
        // 2024-01-01T00:00:00Z .. 2024-02-01T00:00:00Z
        let r = range(1_704_067_200_000_000_000, 1_706_745_600_000_000_000);
        assert_eq!(decode_range(&encode_range(r)), Some(r));
    }

    #[test]
    fn encode_then_decode_round_trips_a_partial_period() {
        // A backfill that only reached the 15th of the month, with a
        // sub-second boundary to prove nanosecond precision survives.
        let r = range(1_704_067_200_000_000_123, 1_705_276_800_000_000_000);
        assert_eq!(decode_range(&encode_range(r)), Some(r));
    }

    #[test]
    fn encode_then_decode_round_trips_an_empty_range() {
        let r = range(0, 0);
        assert_eq!(decode_range(&encode_range(r)), Some(r));
    }

    #[test]
    fn decode_accepts_a_full_filename_with_extension() {
        let r = range(0, 1_000_000_000);
        let filename = format!("{}.parquet", encode_range(r));
        assert_eq!(decode_range(&filename), Some(r));
    }

    #[test]
    fn encoded_tokens_sort_lexicographically_in_chronological_order() {
        let earlier = encode_range(range(0, 1_000_000_000));
        let later = encode_range(range(1_000_000_000, 2_000_000_000));
        assert!(earlier < later, "{earlier} should sort before {later}");
    }

    #[test]
    fn decode_rejects_garbage() {
        assert_eq!(decode_range(""), None);
        assert_eq!(decode_range("not_a_range"), None);
        assert_eq!(decode_range("20240101T000000000000000"), None); // no end half
    }

    #[test]
    fn decode_rejects_an_out_of_range_calendar_field() {
        // Month 13 does not exist.
        let bad = "20241301T000000000000000_20241301T000000000000000";
        assert_eq!(decode_range(bad), None);
    }
}
