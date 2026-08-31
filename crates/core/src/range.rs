//! [`TimeRange`] — a half-open span of time, and the gap planner built on
//! top of it.

use crate::time::UnixNanos;

/// A half-open span of time, `[start, end)`.
///
/// Half-open so adjacent ranges tile without overlap: `[a, b)` followed by
/// `[b, c)` covers `[a, c)` with no double-counted instant and no gap.
///
/// `Hash`: `senken-loader`'s caches and its chunk-keyed
/// single-flight map both key on a `TimeRange` alongside a
/// `SeriesKey`, and both of those already derive `Hash` — deriving it here
/// too, on two plain `i64`-backed fields, needs no wrapper type to carry
/// the same information a second time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TimeRange {
    start: UnixNanos,
    end: UnixNanos,
}

impl TimeRange {
    /// A range from `start` up to (not including) `end`.
    ///
    /// `None` when `end < start`. `end == start` is allowed and describes an
    /// empty range that contains nothing.
    #[must_use]
    pub fn new(start: UnixNanos, end: UnixNanos) -> Option<Self> {
        (end >= start).then_some(Self { start, end })
    }

    /// The inclusive start of the range.
    #[must_use]
    pub fn start(&self) -> UnixNanos {
        self.start
    }

    /// The exclusive end of the range.
    #[must_use]
    pub fn end(&self) -> UnixNanos {
        self.end
    }

    /// `true` when `t` falls inside `[start, end)`.
    #[must_use]
    pub fn contains(&self, t: UnixNanos) -> bool {
        self.start <= t && t < self.end
    }

    /// The overlap between this range and `other`, or `None` when they do
    /// not overlap (including when they merely touch: `[0, 10)` and
    /// `[10, 20)` share no instant).
    #[must_use]
    pub fn intersect(&self, other: &Self) -> Option<Self> {
        let start = self.start.max(other.start);
        let end = self.end.min(other.end);
        (start < end).then_some(Self { start, end })
    }

    /// The gap planner: the parts of this range not covered by any range in
    /// `covered`.
    ///
    /// `covered` may be empty, unsorted, overlapping, or extend outside
    /// this range — all are handled. The result is the minimal set of
    /// non-overlapping ranges, in ascending order, still needed to make
    /// this range fully covered.
    #[must_use]
    pub fn subtract(&self, covered: &[Self]) -> Vec<Self> {
        let mut clipped: Vec<Self> = covered.iter().filter_map(|c| self.intersect(c)).collect();
        clipped.sort_by_key(|r| r.start);

        let mut gaps = Vec::new();
        let mut cursor = self.start;
        for range in clipped {
            if range.start > cursor
                && let Some(gap) = Self::new(cursor, range.start)
            {
                gaps.push(gap);
            }
            if range.end > cursor {
                cursor = range.end;
            }
        }
        if cursor < self.end
            && let Some(gap) = Self::new(cursor, self.end)
        {
            gaps.push(gap);
        }
        gaps
    }
}

#[cfg(test)]
mod tests {
    use super::TimeRange;
    use crate::time::UnixNanos;

    fn range(start: i64, end: i64) -> TimeRange {
        TimeRange::new(UnixNanos::from_nanos(start), UnixNanos::from_nanos(end)).unwrap()
    }

    #[test]
    fn new_rejects_an_end_before_the_start() {
        assert!(TimeRange::new(UnixNanos::from_nanos(10), UnixNanos::from_nanos(5)).is_none());
        assert!(TimeRange::new(UnixNanos::from_nanos(5), UnixNanos::from_nanos(5)).is_some());
    }

    #[test]
    fn contains_is_inclusive_of_start_and_exclusive_of_end() {
        let r = range(0, 100);
        assert!(r.contains(UnixNanos::from_nanos(0)));
        assert!(r.contains(UnixNanos::from_nanos(99)));
        assert!(!r.contains(UnixNanos::from_nanos(100)));
    }

    #[test]
    fn intersect_of_touching_ranges_is_none() {
        assert_eq!(range(0, 10).intersect(&range(10, 20)), None);
    }

    #[test]
    fn intersect_of_overlapping_ranges_is_the_shared_span() {
        assert_eq!(range(0, 10).intersect(&range(5, 20)), Some(range(5, 10)));
    }

    #[test]
    fn intersect_of_disjoint_ranges_is_none() {
        assert_eq!(range(0, 10).intersect(&range(100, 200)), None);
    }

    #[test]
    fn subtract_with_empty_covered_leaves_the_whole_range() {
        assert_eq!(range(0, 100).subtract(&[]), vec![range(0, 100)]);
    }

    #[test]
    fn subtract_with_no_overlap_leaves_the_whole_range() {
        // Coverage entirely outside the requested range contributes nothing.
        assert_eq!(
            range(0, 100).subtract(&[range(200, 300)]),
            vec![range(0, 100)]
        );
    }

    #[test]
    fn subtract_with_partial_overlap_at_the_start_leaves_the_tail() {
        assert_eq!(
            range(0, 100).subtract(&[range(-50, 50)]),
            vec![range(50, 100)]
        );
    }

    #[test]
    fn subtract_with_partial_overlap_at_the_end_leaves_the_head() {
        assert_eq!(
            range(0, 100).subtract(&[range(50, 150)]),
            vec![range(0, 50)]
        );
    }

    #[test]
    fn subtract_with_full_containment_leaves_no_gap() {
        // The covered range extends beyond both ends of the request.
        assert_eq!(range(0, 100).subtract(&[range(-50, 150)]), vec![]);
        // The covered range exactly equals the request.
        assert_eq!(range(0, 100).subtract(&[range(0, 100)]), vec![]);
    }

    #[test]
    fn subtract_treats_adjacent_covered_ranges_as_fully_covering() {
        // [0, 50) and [50, 100) touch with no gap between them, and
        // together fully cover [0, 100) — the merge must not report a
        // zero-width gap at the seam.
        assert_eq!(
            range(0, 100).subtract(&[range(0, 50), range(50, 100)]),
            vec![]
        );
    }

    #[test]
    fn subtract_finds_a_gap_between_two_covered_ranges() {
        assert_eq!(
            range(0, 100).subtract(&[range(0, 20), range(80, 100)]),
            vec![range(20, 80)]
        );
    }

    #[test]
    fn subtract_handles_unsorted_and_overlapping_coverage() {
        // Covered ranges given out of order, and overlapping each other;
        // the result is unaffected by either.
        let covered = [range(60, 90), range(0, 30), range(20, 40)];
        assert_eq!(
            range(0, 100).subtract(&covered),
            vec![range(40, 60), range(90, 100)]
        );
    }
}
