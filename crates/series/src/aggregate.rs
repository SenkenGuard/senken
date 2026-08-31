//! `divides`, `bucket_start` and the streaming [`Aggregator`].
//!
//! # Never emitting a partial bucket
//!
//! A derived M15 needs *complete* coverage of every M1 in its bucket — one
//! missing minute and the high, low and volume are all wrong. The plan
//! leaves the exact mechanism open, on the condition that it is enforced by
//! the type system rather than left to caller convention. This
//! implementation enforces it twice over, on every single push, not just
//! at the edges:
//!
//! 1. **Expected count.** [`Aggregator::new`] rejects any `(source, target)`
//!    pair [`divides`] disagrees with, and derives — never accepts as a raw
//!    caller-supplied number — exactly how many `source` bars one `target`
//!    bucket must contain. For every pair except a fixed-duration source
//!    folding into a [`BarUnit::Month`](crate::BarUnit::Month) target this
//!    count is constant; for that one case (a calendar month is 28–31
//!    days) it is recomputed per bucket from that bucket's own span.
//! 2. **Contiguity.** Every pushed bar must be aligned to `source` and must
//!    immediately follow the previous one — the first bar of a bucket must
//!    land exactly on the bucket's own start. A gap, a duplicate, or an
//!    out-of-order bar poisons the bucket in progress permanently, even if
//!    a coincidental later count would otherwise match. This is what
//!    catches the case count-only tracking cannot: a missing minute and an
//!    unrelated duplicate elsewhere in the same bucket cancelling each
//!    other out.
//!
//! A bucket only ever reaches [`Aggregator::push`]/[`Aggregator::finish`]'s
//! `Some(Bar)` path when both checks agree. There is no third path that
//! returns a bar — [`OpenBucket::close_if_complete`] is the single gate
//! both methods go through.

use senken_core::{UnixNanos, civil_from_days, days_from_civil};

use crate::bar::Bar;
use crate::spec::{BarSpec, BarUnit};

const NANOS_PER_DAY: i64 = 86_400_000_000_000;

/// Defines "midnight" for [`bucket_start`] at [`BarUnit::Day`] and coarser.
///
/// Daily-and-above boundaries are timezone-dependent (UTC midnight vs. an
/// exchange's own close), and because derived series are never persisted
/// the anchor is a property of one aggregation call, not of
/// any [`SeriesKey`](crate::SeriesKey) or stored file: two callers may
/// legally derive the same M1 into UTC-midnight daily bars and
/// exchange-midnight daily bars side by side.
///
/// Ignored below [`BarUnit::Day`] — a minute or an hour boundary needs no
/// notion of "midnight".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    offset_nanos: i64,
}

impl Anchor {
    /// UTC midnight — the default.
    pub const UTC: Self = Self { offset_nanos: 0 };

    /// An anchor whose "midnight" is UTC midnight shifted by `offset_nanos`
    /// (positive: later than UTC midnight; negative: earlier — e.g. an
    /// exchange whose trading day rolls over before UTC midnight).
    #[must_use]
    pub const fn from_offset_nanos(offset_nanos: i64) -> Self {
        Self { offset_nanos }
    }

    /// The raw UTC offset this anchor shifts "midnight" by.
    ///
    /// Exists for `senken-store`: a *venue-supplied*
    /// Day-or-above series is persisted carrying the venue's own anchor
    /// (OKX's plain `1D` opens at UTC+8, for instance), so the anchor is
    /// part of that series' identity and must round-trip through its path
    /// token, or two eight-hour-shifted series would collide under one
    /// name and interleave. Reading the offset back out is what makes that
    /// encoding possible.
    #[must_use]
    pub const fn offset_nanos(self) -> i64 {
        self.offset_nanos
    }
}

impl Default for Anchor {
    fn default() -> Self {
        Self::UTC
    }
}

/// `true` when every `target` bucket, for every value it can ever take, is
/// tileable by whole `source` bars with none left over and none crossing a
/// boundary.
///
/// For every unit but [`BarUnit::Month`] this is a fixed-nanosecond-length
/// question. [`BarUnit::Month`] is not a fixed length (28–31 days), so it
/// gets its own rule: a fixed-duration `source` divides a `Month` target
/// exactly when it divides one calendar day evenly, which is what
/// guarantees it divides *every* whole number of days a month can be made
/// of (weeks do not have this property — a 31-day month is not a multiple
/// of 7 days — so `Week` never divides `Month`). Two `Month`-unit specs
/// divide exactly when their steps do, since an N-month bucket always
/// contains exactly `N / step` whole calendar months regardless of any of
/// their individual lengths.
#[must_use]
pub fn divides(source: BarSpec, target: BarSpec) -> bool {
    match (source.unit, target.unit) {
        (BarUnit::Month, BarUnit::Month) => target.step.get().is_multiple_of(source.step.get()),
        (BarUnit::Month, _) => false,
        (_, BarUnit::Month) => match source.duration_nanos() {
            Some(d) if d > 0 => NANOS_PER_DAY % d == 0,
            _ => false,
        },
        (_, _) => match (source.duration_nanos(), target.duration_nanos()) {
            (Some(s), Some(t)) if s > 0 && t >= s => t % s == 0,
            _ => false,
        },
    }
}

/// The start of the [`spec`](BarSpec)-aligned bucket containing `t`, per
/// `anchor`.
#[must_use]
pub fn bucket_start(t: UnixNanos, spec: BarSpec, anchor: Anchor) -> UnixNanos {
    bucket_bounds(t, spec, anchor).0
}

/// The start of the bucket immediately *after* the one containing `t` — the
/// instant the next bar opens, per `anchor`.
///
/// Shares [`bucket_start`]'s own boundary logic rather than adding
/// "floor `t` to `anchor`, then add one duration" as a second
/// implementation: for a fixed-duration unit those give the same answer,
/// but a caller that reached for the naive version would be one accidental
/// `Anchor::UTC` away from mis-timing every countdown on a venue-anchored
/// Day-or-above series (see this function's own tests).
#[must_use]
pub fn next_bucket_start(t: UnixNanos, spec: BarSpec, anchor: Anchor) -> UnixNanos {
    bucket_bounds(t, spec, anchor).1
}

/// `bucket_start`'s implementation, also used internally by [`Aggregator`]
/// to find a bucket's exclusive end (its length, for [`BarUnit::Month`], is
/// only known by computing where the *next* bucket begins).
fn bucket_bounds(t: UnixNanos, spec: BarSpec, anchor: Anchor) -> (UnixNanos, UnixNanos) {
    let step = i64::from(spec.step.get());
    match spec.unit {
        BarUnit::Second | BarUnit::Minute | BarUnit::Hour => {
            let duration = spec
                .duration_nanos()
                .expect("Second, Minute and Hour always have a fixed duration");
            let n = t.as_nanos();
            let start = n.div_euclid(duration) * duration;
            (
                UnixNanos::from_nanos(start),
                UnixNanos::from_nanos(start + duration),
            )
        }
        BarUnit::Day => {
            let day_index = day_index_of(t, anchor);
            let bucket_day_index = day_index.div_euclid(step) * step;
            (
                day_index_to_ts(bucket_day_index, anchor),
                day_index_to_ts(bucket_day_index + step, anchor),
            )
        }
        BarUnit::Week => {
            let day_index = day_index_of(t, anchor);
            let bucket_monday_index = week_bucket_start_day_index(day_index, step);
            (
                day_index_to_ts(bucket_monday_index, anchor),
                day_index_to_ts(bucket_monday_index + step * 7, anchor),
            )
        }
        BarUnit::Month => {
            let day_index = day_index_of(t, anchor);
            let month_index = month_index_from_day_index(day_index);
            let bucket_month_index = month_index.div_euclid(step) * step;
            (
                day_index_to_ts(month_index_to_day_index(bucket_month_index), anchor),
                day_index_to_ts(month_index_to_day_index(bucket_month_index + step), anchor),
            )
        }
    }
}

/// Days since the epoch of `t`, after shifting by `anchor` so that day
/// boundaries fall where `anchor` defines "midnight".
fn day_index_of(t: UnixNanos, anchor: Anchor) -> i64 {
    (t.as_nanos() - anchor.offset_nanos).div_euclid(NANOS_PER_DAY)
}

/// Inverse of [`day_index_of`]: the instant at the start of `day_index`,
/// shifted back so the result is in real (unshifted) time.
fn day_index_to_ts(day_index: i64, anchor: Anchor) -> UnixNanos {
    UnixNanos::from_nanos(day_index * NANOS_PER_DAY + anchor.offset_nanos)
}

/// The Monday-aligned day index that starts the `step`-week bucket
/// containing `day_index`.
///
/// Weeks are aligned to Monday, matching how `TradingView` and MT5 render a
/// weekly bar — a convention with no basis in the plan or design record
/// (a 24/7 crypto venue has no natural week boundary of its own), chosen
/// here for consistency with mainstream charting rather than derived from
/// any cited fact.
fn week_bucket_start_day_index(day_index: i64, step: i64) -> i64 {
    // 1970-01-01 (day index 0) was a Thursday, so the Monday on or before
    // the epoch is day index -3. Shifting by +3 turns "days since epoch"
    // into "days since that Monday", which floor-divides cleanly into
    // `step`-week blocks.
    const MONDAY_SHIFT: i64 = 3;
    let week_number = (day_index + MONDAY_SHIFT).div_euclid(7);
    let bucket_week_number = week_number.div_euclid(step) * step;
    bucket_week_number * 7 - MONDAY_SHIFT
}

/// An absolute, zero-based month index (month 0 is January of proleptic
/// year 0). The reference point is never observed by a caller — only
/// differences and floor-division by a step matter — so any fixed
/// reference works.
fn month_index_from_day_index(day_index: i64) -> i64 {
    let (year, month, _) = civil_from_days(day_index);
    year * 12 + (month - 1)
}

/// Inverse of [`month_index_from_day_index`], to the 1st of that month.
fn month_index_to_day_index(month_index: i64) -> i64 {
    let year = month_index.div_euclid(12);
    let month = month_index.rem_euclid(12) + 1;
    days_from_civil(year, month, 1)
}

/// Why [`Aggregator::new`] refused to build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum AggregateError {
    /// `finer` does not evenly, unambiguously tile `coarser` — see
    /// [`divides`]. (Named `finer`/`coarser` here, not `source`/`target`:
    /// `thiserror` treats a field literally named `source` as the error's
    /// own [`Error::source`](std::error::Error::source), which would
    /// require `BarSpec` to implement `std::error::Error` for no reason.)
    #[error("{finer} does not evenly divide {coarser}")]
    DoesNotDivide {
        /// The finer spec that was to be folded.
        finer: BarSpec,
        /// The coarser spec that was to be produced.
        coarser: BarSpec,
    },
    /// `coarser` would hold more `finer` bars than fit a `u32` count.
    #[error("{coarser} holds more than u32::MAX {finer} bars")]
    RatioOverflow {
        /// The finer spec that was to be folded.
        finer: BarSpec,
        /// The coarser spec that was to be produced.
        coarser: BarSpec,
    },
}

/// How many `source` bars complete one bucket.
#[derive(Debug, Clone, Copy)]
enum ExpectedCount {
    /// The same for every bucket this aggregator will ever open.
    Fixed(u32),
    /// Recomputed per bucket, from that bucket's own span — only reached
    /// when `target` is [`BarUnit::Month`] and `source` is not, since a
    /// calendar month's length varies (28–31 days) but `source`, having a
    /// fixed duration (required by [`divides`] to reach this case), always
    /// divides it evenly.
    PerBucket,
}

/// A streaming M1-to-N aggregator: folds bars of
/// `source` into bars of `target`, holding at most one bucket in progress.
///
/// See the module docs for exactly how "never emit a partial bucket" is
/// enforced. `push`/`finish` never panic on their input; a malformed or
/// out-of-order bar poisons the bucket in progress (so it will never be
/// emitted) rather than corrupting it silently or aborting the stream.
#[derive(Debug)]
pub struct Aggregator {
    source: BarSpec,
    target: BarSpec,
    anchor: Anchor,
    expected: ExpectedCount,
    open: Option<OpenBucket>,
}

impl Aggregator {
    /// An aggregator folding `source` bars into `target` bars.
    ///
    /// # Errors
    /// [`AggregateError::DoesNotDivide`] unless `divides(source, target)`.
    /// [`AggregateError::RatioOverflow`] in the (practically unreachable
    /// for any real bar spec) case where `target` would hold more than
    /// [`u32::MAX`] `source` bars.
    ///
    /// # Panics
    /// Never in practice: two internal `duration_nanos().expect(..)` calls
    /// exist only to name the invariant that `divides` already checked (a
    /// non-`Month` spec always has a fixed duration), and are unreachable
    /// given the `DoesNotDivide` check above them.
    pub fn new(source: BarSpec, target: BarSpec, anchor: Anchor) -> Result<Self, AggregateError> {
        if !divides(source, target) {
            return Err(AggregateError::DoesNotDivide {
                finer: source,
                coarser: target,
            });
        }
        let expected = if target.unit == BarUnit::Month && source.unit != BarUnit::Month {
            ExpectedCount::PerBucket
        } else {
            let ratio: i64 = if target.unit == BarUnit::Month {
                i64::from(target.step.get()) / i64::from(source.step.get())
            } else {
                let source_duration = source
                    .duration_nanos()
                    .expect("divides() already required source to have a fixed duration here");
                let target_duration = target
                    .duration_nanos()
                    .expect("divides() already required target to have a fixed duration here");
                target_duration / source_duration
            };
            let ratio = u32::try_from(ratio).map_err(|_| AggregateError::RatioOverflow {
                finer: source,
                coarser: target,
            })?;
            ExpectedCount::Fixed(ratio)
        };
        Ok(Self {
            source,
            target,
            anchor,
            expected,
            open: None,
        })
    }

    /// Folds one more `source` bar in, emitting the previous bucket if this
    /// bar starts a new one and the previous bucket was complete.
    ///
    /// Requires non-decreasing `bar.ts_open` across calls (the aggregator
    /// is a streaming, single-pass fold — this matches how bars are always
    /// normalised to ascending order before reaching here). A bar
    /// that arrives for a bucket already passed is dropped and poisons the
    /// bucket currently in progress, since its completeness can no longer
    /// be trusted.
    pub fn push(&mut self, bar: &Bar) -> Option<Bar> {
        let (source_start, source_end) = bucket_bounds(bar.ts_open, self.source, self.anchor);
        let is_source_aligned = source_start == bar.ts_open;
        let (target_start, target_end) = bucket_bounds(bar.ts_open, self.target, self.anchor);

        let Some(open) = self.open.as_mut() else {
            let expected = self.expected_for(target_start, target_end);
            self.open = Some(OpenBucket::start(
                bar,
                target_start,
                expected,
                source_end,
                is_source_aligned && bar.ts_open == target_start,
            ));
            return None;
        };

        if target_start < open.start {
            open.poisoned = true;
            return None;
        }

        if target_start == open.start {
            let contiguous = is_source_aligned && bar.ts_open == open.next_expected_source_start;
            open.absorb(bar, contiguous, source_end);
            return None;
        }

        let emitted = open.close_if_complete();
        let expected = self.expected_for(target_start, target_end);
        self.open = Some(OpenBucket::start(
            bar,
            target_start,
            expected,
            source_end,
            is_source_aligned && bar.ts_open == target_start,
        ));
        emitted
    }

    /// Emits the bucket in progress, if any and if it is complete.
    ///
    /// Calling this on a bucket that is genuinely still forming (the common
    /// case at the head of a live series) correctly returns `None` — a
    /// forming bucket is not a bar yet (a bar is only knowable
    /// at `ts_open + interval`).
    #[must_use]
    pub fn finish(self) -> Option<Bar> {
        self.open.and_then(|open| open.close_if_complete())
    }

    /// How many `source` bars the bucket `[start, end)` must contain.
    fn expected_for(&self, start: UnixNanos, end: UnixNanos) -> u32 {
        match self.expected {
            ExpectedCount::Fixed(n) => n,
            ExpectedCount::PerBucket => {
                let source_duration = self
                    .source
                    .duration_nanos()
                    .expect("PerBucket only occurs when source has a fixed duration");
                let span = end.as_nanos() - start.as_nanos();
                // `u32::MAX` `source` bars in one bucket is not reachable
                // for any spec `Aggregator::new` accepted (the largest
                // realistic case, one-second bars in a one-month bucket,
                // is under 2.7 million); this saturates rather than panics
                // only because `push`/`finish` themselves must not panic.
                u32::try_from(span / source_duration).unwrap_or(u32::MAX)
            }
        }
    }
}

/// One bucket's running fold. `volume`/`quote_volume`/`taker_buy_volume`
/// accumulate in `i128` (a meme token can trade over 1e12
/// units a day, enough to overflow `i64` once folded into a coarse bucket)
/// and are only narrowed back to the `i64` [`Bar`] expects when the bucket
/// closes; an overflow there poisons the whole bar rather than wrapping,
/// exactly like a genuine coverage gap. `trade_count` gets a softer rule:
/// it is metadata, not a price or a quantity, so an overflow there (a
/// practical impossibility at `u32::MAX` trades) degrades that one field
/// to `None` instead of discarding an otherwise-correct OHLCV bar.
#[derive(Debug)]
struct OpenBucket {
    start: UnixNanos,
    expected: u32,
    seen: u32,
    /// Set the moment any absorbed bar breaks alignment, contiguity, or
    /// ordering. Once set, this bucket can never be emitted, regardless of
    /// what `seen` reaches — the count alone cannot tell a genuine run of
    /// `expected` contiguous bars apart from a gap masked by an unrelated
    /// duplicate.
    poisoned: bool,
    /// Where, in `source`-bucket terms, the *next* absorbed bar must start.
    next_expected_source_start: UnixNanos,
    open: i64,
    high: i64,
    low: i64,
    close: i64,
    volume: i128,
    quote_volume: Option<i128>,
    trade_count: Option<u64>,
    taker_buy_volume: Option<i128>,
}

impl OpenBucket {
    fn start(
        bar: &Bar,
        start: UnixNanos,
        expected: u32,
        next_expected_source_start: UnixNanos,
        is_first_slot: bool,
    ) -> Self {
        Self {
            start,
            expected,
            seen: 1,
            poisoned: !is_first_slot,
            next_expected_source_start,
            open: bar.open,
            high: bar.high,
            low: bar.low,
            close: bar.close,
            volume: i128::from(bar.volume),
            quote_volume: bar.quote_volume.map(i128::from),
            trade_count: bar.trade_count.map(u64::from),
            taker_buy_volume: bar.taker_buy_volume.map(i128::from),
        }
    }

    fn absorb(&mut self, bar: &Bar, contiguous: bool, next_expected_source_start: UnixNanos) {
        self.seen += 1;
        self.next_expected_source_start = next_expected_source_start;
        if !contiguous {
            self.poisoned = true;
        }
        if self.poisoned {
            // Still tracked above (`seen`, the boundary) so the aggregator
            // correctly detects when this bucket eventually closes, but a
            // poisoned bucket's OHLCV can never be observed, so there is no
            // point folding it further.
            return;
        }
        self.high = self.high.max(bar.high);
        self.low = self.low.min(bar.low);
        self.close = bar.close;
        self.volume += i128::from(bar.volume);
        self.quote_volume = match (self.quote_volume, bar.quote_volume) {
            (Some(acc), Some(v)) => Some(acc + i128::from(v)),
            _ => None,
        };
        self.trade_count = match (self.trade_count, bar.trade_count) {
            (Some(acc), Some(v)) => Some(acc + u64::from(v)),
            _ => None,
        };
        self.taker_buy_volume = match (self.taker_buy_volume, bar.taker_buy_volume) {
            (Some(acc), Some(v)) => Some(acc + i128::from(v)),
            _ => None,
        };
    }

    /// The single gate every emitted [`Bar`] passes through: complete,
    /// unpoisoned, and every accumulated field back in range.
    fn close_if_complete(&self) -> Option<Bar> {
        if self.poisoned || self.seen != self.expected {
            return None;
        }
        Some(Bar {
            ts_open: self.start,
            open: self.open,
            high: self.high,
            low: self.low,
            close: self.close,
            volume: i64::try_from(self.volume).ok()?,
            quote_volume: match self.quote_volume {
                Some(v) => Some(i64::try_from(v).ok()?),
                None => None,
            },
            trade_count: self.trade_count.and_then(|v| u32::try_from(v).ok()),
            taker_buy_volume: match self.taker_buy_volume {
                Some(v) => Some(i64::try_from(v).ok()?),
                None => None,
            },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{Aggregator, Anchor, divides, next_bucket_start};
    use crate::bar::Bar;
    use crate::spec::{BarSpec, BarUnit};
    use senken_core::UnixNanos;

    fn spec(step: u32, unit: BarUnit) -> BarSpec {
        BarSpec::new(step, unit)
    }

    fn m1_at(minute: i64, open: i64, high: i64, low: i64, close: i64, volume: i64) -> Bar {
        Bar {
            ts_open: UnixNanos::from_secs(minute * 60).unwrap(),
            open,
            high,
            low,
            close,
            volume,
            quote_volume: Some(volume * 10),
            trade_count: Some(1),
            taker_buy_volume: Some(volume / 2),
        }
    }

    #[test]
    fn divides_accepts_m1_into_m15() {
        assert!(divides(spec(1, BarUnit::Minute), spec(15, BarUnit::Minute)));
    }

    #[test]
    fn divides_rejects_a_non_dividing_pair() {
        // 15 is not a multiple of 7.
        assert!(!divides(
            spec(7, BarUnit::Minute),
            spec(15, BarUnit::Minute)
        ));
    }

    #[test]
    fn divides_rejects_a_coarser_source_than_target() {
        assert!(!divides(
            spec(15, BarUnit::Minute),
            spec(1, BarUnit::Minute)
        ));
    }

    #[test]
    fn week_never_divides_month_but_day_always_does() {
        // A 31-day month is not a whole number of 7-day weeks, but every
        // month is a whole number of days by definition.
        assert!(!divides(spec(1, BarUnit::Week), spec(1, BarUnit::Month)));
        assert!(divides(spec(1, BarUnit::Day), spec(1, BarUnit::Month)));
        assert!(divides(spec(1, BarUnit::Minute), spec(1, BarUnit::Month)));
    }

    #[test]
    fn month_multiples_divide_by_their_step_ratio() {
        assert!(divides(spec(1, BarUnit::Month), spec(3, BarUnit::Month)));
        assert!(!divides(spec(2, BarUnit::Month), spec(3, BarUnit::Month)));
    }

    /// Required test: aggregating M1→M5→M15 equals M1→M15 directly.
    #[test]
    fn aggregating_in_two_hops_equals_aggregating_directly() {
        let inputs: Vec<Bar> = (0..15)
            .map(|i| m1_at(i, 100 + i, 110 + i, 90 + i, 105 + i, 1_000 + i))
            .collect();

        // Direct: M1 -> M15.
        let mut direct = Aggregator::new(
            spec(1, BarUnit::Minute),
            spec(15, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap();
        let mut direct_emitted = None;
        for bar in &inputs {
            if let Some(bar) = direct.push(bar) {
                direct_emitted = Some(bar);
            }
        }
        assert!(
            direct_emitted.is_none(),
            "the M15 bucket is not yet closed by any following bar"
        );
        let direct_result = direct
            .finish()
            .expect("all 15 M1 inputs are present and contiguous");

        // Two hops: M1 -> M5, then M5 -> M15.
        let mut m1_to_m5 = Aggregator::new(
            spec(1, BarUnit::Minute),
            spec(5, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap();
        let mut m5_bars = Vec::new();
        for bar in &inputs {
            if let Some(bar) = m1_to_m5.push(bar) {
                m5_bars.push(bar);
            }
        }
        m5_bars.push(
            m1_to_m5
                .finish()
                .expect("the trailing M5 bucket is also complete"),
        );
        assert_eq!(m5_bars.len(), 3, "15 M1 bars fold into exactly 3 M5 bars");

        let mut m5_to_m15 = Aggregator::new(
            spec(5, BarUnit::Minute),
            spec(15, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap();
        let mut two_hop_result = None;
        for bar in &m5_bars {
            if let Some(bar) = m5_to_m15.push(bar) {
                two_hop_result = Some(bar);
            }
        }
        assert!(two_hop_result.is_none());
        let two_hop_result = m5_to_m15
            .finish()
            .expect("all 3 M5 inputs are present and contiguous");

        assert_eq!(direct_result, two_hop_result);
    }

    /// Required test: volume is the sum of inputs; high/low are the
    /// extremes; open is the first input's open and close the last input's
    /// close.
    #[test]
    fn a_derived_bar_combines_its_inputs_correctly() {
        let inputs = [
            m1_at(0, 100, 105, 95, 102, 10),
            m1_at(1, 102, 130, 101, 110, 20), // the extreme high
            m1_at(2, 110, 112, 80, 108, 30),  // the extreme low
            m1_at(3, 108, 109, 107, 106, 40),
            m1_at(4, 106, 107, 104, 99, 50), // the last close
        ];
        let mut agg = Aggregator::new(
            spec(1, BarUnit::Minute),
            spec(5, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap();
        for bar in &inputs {
            assert!(
                agg.push(bar).is_none(),
                "a 5-input M5 bucket never closes mid-stream here"
            );
        }
        let bar = agg.finish().unwrap();

        assert_eq!(bar.open, 100, "open is the first input's open");
        assert_eq!(bar.close, 99, "close is the last input's close");
        assert_eq!(bar.high, 130, "high is the extreme of every input's high");
        assert_eq!(bar.low, 80, "low is the extreme of every input's low");
        assert_eq!(
            bar.volume,
            10 + 20 + 30 + 40 + 50,
            "volume is the sum of every input"
        );
        assert_eq!(
            bar.quote_volume,
            Some((10 + 20 + 30 + 40 + 50) * 10),
            "quote_volume sums the same way as volume"
        );
        assert_eq!(bar.trade_count, Some(5), "one trade per input bar here");
    }

    /// Required test: `i128` accumulation is rejected on overflow, not
    /// wrapped, when narrowed back to the `i64` `Bar` expects.
    #[test]
    fn volume_overflow_rejects_the_bar_instead_of_wrapping() {
        let inputs = [
            Bar {
                volume: i64::MAX,
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
                ..m1_at(0, 1, 1, 1, 1, 0)
            },
            Bar {
                volume: i64::MAX,
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
                ..m1_at(1, 1, 1, 1, 1, 0)
            },
        ];
        // i64::MAX * 2 comfortably overflows i64 but is nowhere near
        // i128::MAX, so the *sum* itself never wraps mid-stream — only the
        // final narrowing back to i64 can fail, and must be caught there.
        let mut agg = Aggregator::new(
            spec(1, BarUnit::Minute),
            spec(2, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap();
        for bar in &inputs {
            assert!(agg.push(bar).is_none());
        }
        assert_eq!(
            agg.finish(),
            None,
            "an i64-overflowing volume must be rejected, not silently wrapped into a wrong bar"
        );
    }

    /// Required test: a partial bucket is provably never emitted — the
    /// plain case (too few inputs).
    #[test]
    fn a_bucket_with_too_few_inputs_is_never_emitted() {
        let inputs = [
            m1_at(0, 1, 1, 1, 1, 1),
            m1_at(1, 1, 1, 1, 1, 1),
            m1_at(2, 1, 1, 1, 1, 1),
        ];
        let mut agg = Aggregator::new(
            spec(1, BarUnit::Minute),
            spec(5, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap();
        for bar in &inputs {
            assert!(agg.push(bar).is_none());
        }
        assert_eq!(
            agg.finish(),
            None,
            "only 3 of the 5 required M1 inputs were ever pushed"
        );
    }

    /// Required test: a partial bucket is provably never emitted — the
    /// adversarial case, where a gap is masked by an unrelated duplicate so
    /// the raw *count* alone would look complete.
    #[test]
    fn a_gap_masked_by_a_duplicate_is_never_emitted_despite_a_matching_count() {
        // Minute 2 is missing; minute 1 is pushed twice instead. Five
        // pushes total for a bucket that needs five M1 inputs — the count
        // alone cannot tell this apart from a genuine 0..5 run.
        let inputs = [
            m1_at(0, 1, 1, 1, 1, 1),
            m1_at(1, 1, 1, 1, 1, 1),
            m1_at(1, 1, 1, 1, 1, 1), // duplicate of minute 1, not minute 2
            m1_at(3, 1, 1, 1, 1, 1),
            m1_at(4, 1, 1, 1, 1, 1),
        ];
        let mut agg = Aggregator::new(
            spec(1, BarUnit::Minute),
            spec(5, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap();
        for bar in &inputs {
            assert!(agg.push(bar).is_none());
        }
        assert_eq!(
            agg.finish(),
            None,
            "five pushes with a gap and a masking duplicate must not be mistaken for five contiguous inputs"
        );
    }

    #[test]
    fn a_bucket_completed_by_a_following_bars_arrival_is_emitted_from_push_not_finish() {
        let inputs: Vec<Bar> = (0..6).map(|i| m1_at(i, 1, 1, 1, 1, 1)).collect();
        let mut agg = Aggregator::new(
            spec(1, BarUnit::Minute),
            spec(5, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap();
        let mut emitted = Vec::new();
        for bar in &inputs {
            if let Some(bar) = agg.push(bar) {
                emitted.push(bar);
            }
        }
        assert_eq!(
            emitted.len(),
            1,
            "minute 5 starting a new bucket must close the first one"
        );
        assert_eq!(emitted[0].ts_open, UnixNanos::from_secs(0).unwrap());
    }

    #[test]
    fn an_out_of_order_bar_is_dropped_and_does_not_corrupt_the_open_bucket() {
        let mut agg = Aggregator::new(
            spec(1, BarUnit::Minute),
            spec(5, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap();
        assert!(agg.push(&m1_at(2, 1, 1, 1, 1, 1)).is_none());
        // Minute 0 arriving after minute 2 is out of order for a streaming,
        // single-pass fold.
        assert!(agg.push(&m1_at(0, 1, 1, 1, 1, 1)).is_none());
        // The bucket in progress is now poisoned and can never complete,
        // regardless of what arrives next.
        for bar in (3..6).map(|i| m1_at(i, 1, 1, 1, 1, 1)) {
            agg.push(&bar);
        }
        assert_eq!(agg.finish(), None);
    }

    #[test]
    fn a_day_bucket_respects_a_non_utc_anchor() {
        // An exchange whose day rolls over at 17:00 UTC. A bar timestamped
        // 18:00 UTC on day 0 belongs to *day 1* of that exchange's
        // calendar, one hour past its midnight.
        let anchor = Anchor::from_offset_nanos(17 * 3_600_000_000_000);
        let day_spec = spec(1, BarUnit::Day);
        let t = UnixNanos::from_secs(18 * 3600).unwrap(); // 1970-01-01T18:00:00Z
        let start = super::bucket_start(t, day_spec, anchor);
        assert_eq!(start, UnixNanos::from_secs(17 * 3600).unwrap());

        // The same instant under the UTC anchor still belongs to day 0.
        let utc_start = super::bucket_start(t, day_spec, Anchor::UTC);
        assert_eq!(utc_start, UnixNanos::EPOCH);
    }

    #[test]
    fn next_bucket_start_respects_a_non_utc_day_anchor() {
        // Same exchange as `a_day_bucket_respects_a_non_utc_anchor`: its
        // trading day rolls over at 17:00 UTC, not UTC midnight — the exact
        // shape OKX's plain `1D` (opens at UTC+8) versus `1Dutc` carries. A
        // countdown built by flooring "now" to UTC midnight and adding one
        // day would report 1970-01-02T00:00:00Z here — two hours early.
        let anchor = Anchor::from_offset_nanos(17 * 3_600_000_000_000);
        let day_spec = spec(1, BarUnit::Day);
        let t = UnixNanos::from_secs(18 * 3600).unwrap(); // 1970-01-01T18:00:00Z

        let next = super::next_bucket_start(t, day_spec, anchor);

        assert_eq!(next, UnixNanos::from_secs(17 * 3600 + 86_400).unwrap());
    }

    #[test]
    fn a_week_bucket_starts_on_monday() {
        // 1970-01-05 is a Monday (epoch, 1970-01-01, was a Thursday).
        let monday = UnixNanos::from_secs(4 * 86_400).unwrap();
        let start = super::bucket_start(monday, spec(1, BarUnit::Week), Anchor::UTC);
        assert_eq!(start, monday);

        // A Wednesday in the same week must resolve to the same Monday.
        let wednesday = UnixNanos::from_secs(6 * 86_400).unwrap();
        assert_eq!(
            super::bucket_start(wednesday, spec(1, BarUnit::Week), Anchor::UTC),
            monday
        );
    }

    #[test]
    fn a_month_bucket_starts_on_the_1st_regardless_of_month_length() {
        // 2026-08-30 (31-day August) and 2026-02-15 (28-day February)
        // must each resolve to the 1st of their own month.
        let aug_30 = UnixNanos::from_secs(1_788_048_000).unwrap();
        let aug_1 = UnixNanos::from_secs(1_788_048_000 - 29 * 86_400).unwrap();
        assert_eq!(
            super::bucket_start(aug_30, spec(1, BarUnit::Month), Anchor::UTC),
            aug_1
        );
    }

    #[test]
    fn aggregator_new_rejects_a_pair_that_does_not_divide() {
        let err = Aggregator::new(
            spec(7, BarUnit::Minute),
            spec(15, BarUnit::Minute),
            Anchor::UTC,
        )
        .unwrap_err();
        assert_eq!(
            err,
            super::AggregateError::DoesNotDivide {
                finer: spec(7, BarUnit::Minute),
                coarser: spec(15, BarUnit::Minute),
            }
        );
    }

    #[test]
    fn the_next_bucket_start_of_an_intraday_spec_is_the_next_step_boundary() {
        // 00:10:30 on a 15m grid -> 00:15:00.
        let t = UnixNanos::from_secs(630).unwrap();
        let next = next_bucket_start(t, spec(15, BarUnit::Minute), Anchor::UTC);

        assert_eq!(next, UnixNanos::from_secs(900).unwrap());
    }
}
