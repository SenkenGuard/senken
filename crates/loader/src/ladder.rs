//! The resolution ladder: memory cache → store at
//! the exact spec → aggregate from stored finer specs that divide evenly
//! and are aligned (coarsest first, stitching adjacent coverage together
//! when no single one spans the whole request) → whatever remains is
//! genuinely missing.
//!
//! This module never fetches. [`compute_gap`] is exactly what
//! [`crate::SeriesLoader::plan`] needs ("pure inspection —
//! touches no network, starts no work, mutates nothing") and reads only
//! coverage — never a Parquet file's rows — which is also what
//! [`crate::SeriesLoader::ensure`] fetches to fill. Actually reading bar
//! content happens once, in [`materialize`], kept deliberately separate so
//! `plan()` can never accidentally decode a row it does not need to.

use std::sync::Arc;

use senken_core::TimeRange;
use senken_series::{Aggregator, Anchor, Bar, BarSpec, Origin, SeriesKey, bucket_start, divides};
use senken_store::Store;

use crate::cache::{BarCache, CachedBars};
use crate::coverage::CoverageCache;
use crate::error::LoadError;
use crate::generation::GenerationTracker;

/// What the ladder found without fetching anything (steps 1–3).
#[derive(Debug, Clone)]
pub(crate) struct GapPlan {
    /// The parts of the request already resolvable from disk, each tagged
    /// with the [`Origin::Venue`] spec it was folded from — `None` for an
    /// [`Origin::Venue`] request, since nothing is folded; `Some(spec)` for
    /// every piece of an [`Origin::Derived`] one (a stitched
    /// request may credit different pieces to different candidate specs,
    /// so this is per-piece, not one value for the whole plan).
    pub(crate) covered: Vec<(TimeRange, Option<BarSpec>)>,
    /// The parts that genuinely need a fetch — always expressed as gaps in
    /// the fetch spec's own coverage (`key.spec` for an
    /// [`Origin::Venue`] request, [`Candidates::base_spec`] for an
    /// [`Origin::Derived`] one), since that is what a fetch would actually
    /// run at.
    pub(crate) missing: Vec<TimeRange>,
}

/// The finer specs the ladder may find already stored (`Origin::Venue`) for
/// a symbol, plus the base spec a fetch actually runs at.
pub(crate) struct Candidates {
    /// The finest spec this loader ever fetches from a
    /// [`crate::BarSource`] directly. Always a legal candidate — even with
    /// `finer_specs` empty, a `Derived` request can still be resolved by
    /// fetching and folding this spec.
    pub(crate) base_spec: BarSpec,
    /// Specs the ladder should look for already stored before falling back
    /// to `base_spec`, in no particular order — [`Self::ordered_for`]
    /// sorts them.
    pub(crate) finer_specs: Vec<BarSpec>,
}

impl Candidates {
    /// Every candidate that genuinely divides `target`, coarsest — fewest
    /// rows to fold — first. Identity is deliberately included: a stored
    /// venue series at the requested spec is authoritative and needs no
    /// aggregation.
    fn ordered_for(&self, target: BarSpec) -> Vec<BarSpec> {
        let mut candidates: Vec<BarSpec> = self
            .finer_specs
            .iter()
            .copied()
            .chain(std::iter::once(self.base_spec))
            .filter(|candidate| divides(*candidate, target))
            .collect();
        candidates.sort_by_key(|c| std::cmp::Reverse(c.duration_nanos()));
        candidates.dedup();
        candidates
    }

    /// The coarsest venue-supported spec that divides `target`, falling
    /// back to the canonical base when the venue has no compatible spec.
    pub(crate) fn fetch_spec_for(&self, target: BarSpec) -> BarSpec {
        self.ordered_for(target)
            .into_iter()
            .next()
            .unwrap_or(self.base_spec)
    }
}

/// The [`SeriesKey`] for `key`'s symbol/source at a stored, [`Origin::Venue`]
/// `spec` — the identity every candidate finer series is looked up under
/// (only venue-supplied specs are ever persisted).
pub(crate) fn venue_key(key: &SeriesKey, spec: BarSpec) -> SeriesKey {
    SeriesKey::new(
        key.source_id.clone(),
        key.symbol.clone(),
        Origin::Venue,
        spec,
    )
}

/// Computes [`GapPlan`] for `key`/`range`, reading nothing but coverage
/// (via `coverage_cache`, itself backed by [`Store::coverage`] — a
/// directory listing.1 — never an opened file).
///
/// For an [`Origin::Derived`] key, no single candidate needs to cover the
/// *entire* requested range any more: candidates
/// are tried coarsest first, each one credited for whatever whole,
/// bucket-aligned portion of the still-unresolved range it actually covers
/// (never a partial bucket — see [`trim_to_whole_buckets`]), and the next
/// candidate is tried against whatever is left. Two adjacent, independently
/// fully-covered regions (e.g. an hour stored at `venue-15m`, the next hour
/// stored at `venue-1m`) therefore both resolve, where the pre-M8.5 ladder
/// would have rejected both and re-fetched the base spec for the whole
/// range. **The completeness rule stays absolute**: a candidate is only
/// ever credited for whole buckets it fully backs, so a bucket that cannot
/// be completed from what is on disk is always left in `missing`, never
/// silently dropped — see [`trim_to_whole_buckets`]'s own docs.
pub(crate) fn compute_gap(
    store: &Store,
    coverage_cache: &CoverageCache,
    candidates: &Candidates,
    key: &SeriesKey,
    range: TimeRange,
    anchor: Anchor,
) -> Result<GapPlan, LoadError> {
    if range.start() == range.end() {
        return Ok(GapPlan {
            covered: Vec::new(),
            missing: Vec::new(),
        });
    }

    match key.origin {
        Origin::Venue => {
            let covered_ranges = coverage_cache.get(store, key, anchor)?;
            let missing = range.subtract(&covered_ranges);
            let covered = if missing.is_empty() {
                vec![(range, None)]
            } else {
                let mut c: Vec<(TimeRange, Option<BarSpec>)> = covered_ranges
                    .into_iter()
                    .filter_map(|c| c.intersect(&range))
                    .map(|r| (r, None))
                    .collect();
                c.sort_by_key(|(r, _)| r.start());
                c
            };
            Ok(GapPlan { covered, missing })
        }
        Origin::Derived => {
            // Stitching needs `key.spec`'s bucket length to trim a
            // candidate's raw file coverage down to whole buckets
            // (`trim_to_whole_buckets`) — undefined for `BarUnit::Month`,
            // whose buckets vary 28–31 days (the same case its own
            // `Aggregator` special-cases). For a `Month` target this ladder
            // keeps the pre-M8.5 rule: a single candidate must cover the
            // whole requested range, or nothing on disk is used at all.
            if key.spec.duration_nanos().is_none() {
                return compute_gap_whole_range_only(
                    store,
                    coverage_cache,
                    candidates,
                    key,
                    range,
                    anchor,
                );
            }

            let mut remaining = vec![range];
            let mut covered: Vec<(TimeRange, Option<BarSpec>)> = Vec::new();
            for candidate in candidates.ordered_for(key.spec) {
                if remaining.is_empty() {
                    break;
                }
                let candidate_key = venue_key(key, candidate);
                let candidate_coverage = coverage_cache.get(store, &candidate_key, anchor)?;
                remaining = fold_candidate(
                    &mut covered,
                    remaining,
                    &candidate_coverage,
                    candidate,
                    key.spec,
                    anchor,
                );
            }
            if !remaining.is_empty() {
                let fetch_spec = candidates.fetch_spec_for(key.spec);
                let fetch_key = venue_key(key, fetch_spec);
                let fetch_coverage = coverage_cache.get(store, &fetch_key, anchor)?;
                remaining = fold_candidate(
                    &mut covered,
                    remaining,
                    &fetch_coverage,
                    fetch_spec,
                    key.spec,
                    anchor,
                );
            }
            covered.sort_by_key(|(r, _)| r.start());
            Ok(GapPlan {
                covered,
                missing: remaining,
            })
        }
    }
}

/// The pre-M8.5 rule, kept for the one case stitching cannot handle
/// (`key.spec.unit == BarUnit::Month`, see [`compute_gap`]'s own docs): a
/// single candidate must cover the *entire* requested range to be used at
/// all, or the whole range is reported missing at `candidates.base_spec`.
fn compute_gap_whole_range_only(
    store: &Store,
    coverage_cache: &CoverageCache,
    candidates: &Candidates,
    key: &SeriesKey,
    range: TimeRange,
    anchor: Anchor,
) -> Result<GapPlan, LoadError> {
    for candidate in candidates.ordered_for(key.spec) {
        let candidate_key = venue_key(key, candidate);
        let covered_ranges = coverage_cache.get(store, &candidate_key, anchor)?;
        if range.subtract(&covered_ranges).is_empty() {
            return Ok(GapPlan {
                covered: vec![(range, Some(candidate))],
                missing: Vec::new(),
            });
        }
    }
    let fetch_spec = candidates.fetch_spec_for(key.spec);
    let fetch_key = venue_key(key, fetch_spec);
    let fetch_covered = coverage_cache.get(store, &fetch_key, anchor)?;
    let missing = range.subtract(&fetch_covered);
    Ok(GapPlan {
        covered: Vec::new(),
        missing,
    })
}

/// Consumes as much of `remaining` as `coverage` (one candidate spec's raw,
/// file-declared ranges) can resolve, tagging each newly-credited piece
/// with `spec` in `covered`; returns whatever is still left afterward.
///
/// Only whole, `target`-bucket-aligned pieces are ever credited — see
/// [`trim_to_whole_buckets`]. A partial bucket at either edge of `coverage`
/// is deliberately left in the returned "still missing" set rather than
/// silently accepted, which is what keeps the completeness rule (plan
/// M8.5: "a bucket with incomplete coverage is never served as a complete
/// derived bar") from becoming a promise this function alone cannot back:
/// crediting a sliver too thin to complete a bucket would report that time
/// as resolved (dropping it from `missing`, so [`crate::SeriesLoader::ensure`]
/// never fetches it) while [`materialize`] would still correctly refuse to
/// emit a bar for it — leaving a permanent, silent hole.
fn fold_candidate(
    covered: &mut Vec<(TimeRange, Option<BarSpec>)>,
    remaining: Vec<TimeRange>,
    coverage: &[TimeRange],
    spec: BarSpec,
    target: BarSpec,
    anchor: Anchor,
) -> Vec<TimeRange> {
    let mut still_missing = Vec::new();
    for segment in remaining {
        let credited: Vec<TimeRange> = coverage
            .iter()
            .filter_map(|c| c.intersect(&segment))
            .filter_map(|piece| trim_to_whole_buckets(piece, target, anchor))
            .collect();
        for piece in &credited {
            covered.push((*piece, Some(spec)));
        }
        still_missing.extend(segment.subtract(&credited));
    }
    still_missing
}

/// Trims `piece` inward to the largest sub-range that is an exact, whole
/// number of `target`-spec buckets under `anchor` — dropping a partial
/// bucket at either edge rather than crediting it. `None` when nothing
/// whole is left (`piece` itself is narrower than one bucket, or exactly
/// spans a single partial one).
///
/// This is what stops stitching from ever accepting a sliver of candidate
/// coverage too thin to complete a bucket on its own — see
/// [`fold_candidate`]'s docs for why that matters.
///
/// # Panics
/// Never: only called for `target`s with a fixed [`BarSpec::duration_nanos`]
/// (`compute_gap` routes `BarUnit::Month` targets to
/// [`compute_gap_whole_range_only`] instead, which never calls this).
fn trim_to_whole_buckets(piece: TimeRange, target: BarSpec, anchor: Anchor) -> Option<TimeRange> {
    let duration = target
        .duration_nanos()
        .expect("trim_to_whole_buckets is never called for a Month-unit target");
    let step = std::time::Duration::from_nanos(u64::try_from(duration).ok()?);

    let start_bucket = bucket_start(piece.start(), target, anchor);
    let start = if start_bucket == piece.start() {
        piece.start()
    } else {
        // `piece.start()` falls inside a bucket that began before it —
        // that bucket is not fully backed by `piece`, so it is excluded,
        // not credited.
        start_bucket.checked_add(step)?
    };

    // The bucket containing `piece.end()` is only fully inside `piece` when
    // `piece.end()` itself sits exactly on that bucket's start (the range
    // is half-open); either way, the start of that bucket is exactly the
    // exclusive end of the last bucket `piece` fully backs.
    let end = bucket_start(piece.end(), target, anchor);

    TimeRange::new(start, end).filter(|r| r.start() < r.end())
}

/// Reads `key`'s bars over `range` from `store`, decoded once per
/// `(key, range)` and cached thereafter (the "page cache" role of
/// [`BarCache`] — `derived_from: Vec::new()`, since nothing was folded to
/// produce a direct store read).
pub(crate) fn read_store_bars(
    store: &Store,
    bar_cache: &BarCache,
    generations: &GenerationTracker,
    key: &SeriesKey,
    anchor: Anchor,
    range: TimeRange,
) -> Result<Arc<[Bar]>, LoadError> {
    if let Some(cached) = bar_cache.get(key, range, generations) {
        return Ok(cached.bars);
    }
    let mut bars = Vec::new();
    for batch in store.read_range(key, anchor, range)? {
        bars.extend(senken_store::bars_from_batch(&batch?)?);
    }
    // A returned batch's row group may span beyond the exact query
    // boundary (`Store::read_range`'s own docs) — trim to what was asked.
    bars.retain(|b| range.contains(b.ts_open));
    let bars: Arc<[Bar]> = Arc::from(bars.into_boxed_slice());
    bar_cache.insert(
        key.clone(),
        range,
        CachedBars {
            bars: Arc::clone(&bars),
            derived_from: Vec::new(),
        },
    );
    Ok(bars)
}

/// Folds `source_bars` (all of `from_spec`, ascending) into `target` bars,
/// never emitting a bucket [`Aggregator`] cannot prove complete — design
/// its non-negotiable rule, enforced by `senken-series` itself; this
/// function adds no leniency on top of it.
fn aggregate_bars(
    source_bars: &[Bar],
    from_spec: BarSpec,
    target: BarSpec,
    anchor: Anchor,
) -> Result<Vec<Bar>, LoadError> {
    let mut aggregator = Aggregator::new(from_spec, target, anchor)?;
    let mut out = Vec::new();
    for bar in source_bars {
        if let Some(emitted) = aggregator.push(bar) {
            out.push(emitted);
        }
    }
    if let Some(emitted) = aggregator.finish() {
        out.push(emitted);
    }
    Ok(out)
}

/// Turns a [`GapPlan`] into the bars a caller of
/// [`crate::SeriesLoader::resolve`] actually wants, reading/aggregating
/// only the `covered` portion `compute_gap` already proved resolvable.
/// Never called for `plan()` — this is the half that touches real bar
/// content and belongs only on the read path.
///
/// A stitched [`Origin::Derived`] request may fold more than one candidate
/// spec — each `covered` segment is aggregated independently,
/// through its own [`Aggregator`], since mixing bars of two different specs
/// into one aggregator instance is not meaningful. The combined result is
/// cached under the *whole originally requested* `range` only when `gap`
/// leaves nothing missing (`gap.missing.is_empty()`): caching a partial
/// result under the full range would make [`crate::SeriesLoader::resolve`]'s
/// own cache-hit check — which reports `missing: Vec::new()` on a hit —
/// silently imply a completeness that is not real.
pub(crate) fn materialize(
    store: &Store,
    bar_cache: &BarCache,
    generations: &GenerationTracker,
    key: &SeriesKey,
    anchor: Anchor,
    range: TimeRange,
    gap: &GapPlan,
) -> Result<Vec<Bar>, LoadError> {
    if gap.covered.is_empty() {
        return Ok(Vec::new());
    }
    match key.origin {
        Origin::Venue => {
            let mut bars = Vec::new();
            for (covered, _) in &gap.covered {
                bars.extend_from_slice(&read_store_bars(
                    store,
                    bar_cache,
                    generations,
                    key,
                    anchor,
                    *covered,
                )?);
            }
            bars.sort_by_key(|b| b.ts_open);
            Ok(bars)
        }
        Origin::Derived => {
            let mut bars = Vec::new();
            let mut deps: Vec<(SeriesKey, u64)> = Vec::new();
            for (segment, from_spec) in &gap.covered {
                let Some(from_spec) = *from_spec else {
                    // Never produced for a `Derived` gap (`compute_gap`
                    // always tags a `Derived` segment with `Some`) — kept
                    // as a skip rather than an `unreachable!` so a future
                    // change to `GapPlan` construction fails safe.
                    continue;
                };
                let candidate_key = venue_key(key, from_spec);
                let source_bars = read_store_bars(
                    store,
                    bar_cache,
                    generations,
                    &candidate_key,
                    anchor,
                    *segment,
                )?;
                deps.push((candidate_key.clone(), generations.current(&candidate_key)));
                if from_spec == key.spec {
                    bars.extend_from_slice(&source_bars);
                } else {
                    bars.extend(aggregate_bars(&source_bars, from_spec, key.spec, anchor)?);
                }
            }
            bars.sort_by_key(|b| b.ts_open);
            if gap.missing.is_empty() {
                let derived_arc: Arc<[Bar]> = Arc::from(bars.clone().into_boxed_slice());
                bar_cache.insert(
                    key.clone(),
                    range,
                    CachedBars {
                        bars: derived_arc,
                        derived_from: deps,
                    },
                );
            }
            Ok(bars)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Candidates, compute_gap, materialize};
    use crate::cache::BarCache;
    use crate::coverage::CoverageCache;
    use crate::generation::GenerationTracker;
    use senken_core::{TimeRange, UnixNanos};
    use senken_series::{Anchor, Bar, BarSpec, BarUnit, Origin, SeriesKey};
    use senken_store::Store;
    use tempfile::TempDir;

    fn bar(ts_open_secs: i64, close: i64, volume: i64) -> Bar {
        Bar {
            ts_open: UnixNanos::from_secs(ts_open_secs).unwrap(),
            open: close,
            high: close,
            low: close,
            close,
            volume: senken_series::Volume::Real(volume),
            quote_volume: None,
            trade_count: None,
            taker_buy_volume: None,
        }
    }

    fn derived_key(spec: BarSpec) -> SeriesKey {
        SeriesKey::new("binance-spot", "BTCUSDT", Origin::Derived, spec)
    }

    fn secs_range(start: i64, end: i64) -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(start).unwrap(),
            UnixNanos::from_secs(end).unwrap(),
        )
        .unwrap()
    }

    /// `compute_gap` then `materialize`, bundled — every test below wants
    /// exactly this pair, and spelling both out inline at every call site
    /// is what was making the stitching test unreasonably long.
    fn resolve(
        store: &Store,
        coverage_cache: &CoverageCache,
        candidates: &Candidates,
        bar_cache: &BarCache,
        generations: &GenerationTracker,
        key: &SeriesKey,
        range: TimeRange,
    ) -> Vec<Bar> {
        let gap = compute_gap(store, coverage_cache, candidates, key, range, Anchor::UTC).unwrap();
        materialize(store, bar_cache, generations, key, Anchor::UTC, range, &gap).unwrap()
    }

    #[test]
    fn venue_identity_is_preferred_over_finer_stored_series() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let m1 = BarSpec::new(1, BarUnit::Minute);
        let h1 = BarSpec::new(1, BarUnit::Hour);
        let range = secs_range(0, 3600);
        let key = derived_key(h1);
        store
            .write(
                &super::venue_key(&key, h1),
                Anchor::UTC,
                0,
                0,
                range,
                &[bar(0, 7, 60)],
            )
            .unwrap();
        let minute_bars: Vec<Bar> = (0..60).map(|i| bar(i * 60, 3, 1)).collect();
        store
            .write(
                &super::venue_key(&key, m1),
                Anchor::UTC,
                0,
                0,
                range,
                &minute_bars,
            )
            .unwrap();

        let candidates = Candidates {
            base_spec: m1,
            finer_specs: vec![m1, h1],
        };
        let gap = compute_gap(
            &store,
            &CoverageCache::default(),
            &candidates,
            &key,
            range,
            Anchor::UTC,
        )
        .unwrap();
        assert_eq!(gap.covered, vec![(range, Some(h1))]);
        assert_eq!(gap.missing, Vec::new());
        let bars = materialize(
            &store,
            &BarCache::new(usize::MAX),
            &GenerationTracker::default(),
            &key,
            Anchor::UTC,
            range,
            &gap,
        )
        .unwrap();
        assert_eq!(bars, vec![bar(0, 7, 60)]);
    }

    #[test]
    fn fetch_spec_is_the_coarsest_supported_spec_that_divides_the_target() {
        let m1 = BarSpec::new(1, BarUnit::Minute);
        let h1 = BarSpec::new(1, BarUnit::Hour);
        let h4 = BarSpec::new(4, BarUnit::Hour);
        let candidates = Candidates {
            base_spec: m1,
            finer_specs: vec![m1, h1],
        };

        assert_eq!(candidates.fetch_spec_for(h4), h1);
    }

    #[test]
    fn fetch_spec_rejects_a_supported_spec_that_does_not_divide_the_target() {
        let m1 = BarSpec::new(1, BarUnit::Minute);
        let h3 = BarSpec::new(3, BarUnit::Hour);
        let h4 = BarSpec::new(4, BarUnit::Hour);
        let candidates = Candidates {
            base_spec: m1,
            finer_specs: vec![h3],
        };

        assert_eq!(candidates.fetch_spec_for(h4), m1);
    }

    #[test]
    fn fetch_spec_preserves_the_base_fallback_when_no_candidate_divides() {
        let m1 = BarSpec::new(1, BarUnit::Minute);
        let seconds_7 = BarSpec::new(7, BarUnit::Second);
        let seconds_11 = BarSpec::new(11, BarUnit::Second);
        let candidates = Candidates {
            base_spec: m1,
            finer_specs: vec![seconds_11],
        };

        assert_eq!(candidates.fetch_spec_for(seconds_7), m1);
    }

    /// Plan M6, required test: "the ladder prefers the coarsest fully
    /// covering candidate". Both M1 and M15 fully cover the requested hour,
    /// but their underlying values disagree — proving which one actually
    /// got used.
    #[test]
    fn the_ladder_prefers_the_coarsest_fully_covering_candidate() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let hour = secs_range(0, 3600);
        let m1 = BarSpec::new(1, BarUnit::Minute);
        let m15 = BarSpec::new(15, BarUnit::Minute);
        let h1 = BarSpec::new(1, BarUnit::Hour);

        // M1: 60 bars, all close = 1 -> folded H1 close would be 1.
        let minute_bars: Vec<Bar> = (0..60).map(|i| bar(i * 60, 1, 1)).collect();
        store
            .write(
                &super::venue_key(&derived_key(h1), m1),
                Anchor::UTC,
                0,
                0,
                hour,
                &minute_bars,
            )
            .unwrap();

        // M15: 4 bars, deliberately disagreeing with the M1 fold (close = 2)
        // so the resulting its `close` reveals which spec was used.
        let quarter_hour_bars: Vec<Bar> = (0..4).map(|i| bar(i * 900, 2, 1)).collect();
        store
            .write(
                &super::venue_key(&derived_key(h1), m15),
                Anchor::UTC,
                0,
                0,
                hour,
                &quarter_hour_bars,
            )
            .unwrap();

        let candidates = Candidates {
            base_spec: m1,
            finer_specs: vec![m1, m15],
        };
        let coverage_cache = CoverageCache::default();
        let key = derived_key(h1);
        let gap = compute_gap(
            &store,
            &coverage_cache,
            &candidates,
            &key,
            hour,
            Anchor::UTC,
        )
        .unwrap();
        assert_eq!(
            gap.covered,
            vec![(hour, Some(m15))],
            "M15 is coarser than M1 and fully covers the range"
        );
        assert_eq!(gap.missing, Vec::new());

        let bar_cache = BarCache::new(usize::MAX);
        let generations = GenerationTracker::default();
        let bars = materialize(
            &store,
            &bar_cache,
            &generations,
            &key,
            Anchor::UTC,
            hour,
            &gap,
        )
        .unwrap();
        assert_eq!(bars.len(), 1);
        assert_eq!(
            bars[0].close, 2,
            "the derived bar must come from folding M15, not M1"
        );
    }

    /// Plan M6, required test: "an incomplete bucket is never served as a
    /// complete derived bar." Coverage (the filename) says the full hour of
    /// M1 was fetched, but one minute's row is missing inside it (a real
    /// market gap) — the H1 bucket must not be emitted.
    #[test]
    fn an_incomplete_bucket_is_never_served_as_a_complete_derived_bar() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let m1 = BarSpec::new(1, BarUnit::Minute);
        let h1 = BarSpec::new(1, BarUnit::Hour);
        let two_hours = secs_range(0, 7200);

        // First hour: complete 60 bars. Second hour: minute 30 missing.
        let mut m1_bars: Vec<Bar> = (0..60).map(|i| bar(i * 60, 1, 1)).collect();
        m1_bars.extend((60..120).filter(|&i| i != 90).map(|i| bar(i * 60, 1, 1)));
        store
            .write(
                &super::venue_key(&derived_key(h1), m1),
                Anchor::UTC,
                0,
                0,
                two_hours,
                &m1_bars,
            )
            .unwrap();

        let candidates = Candidates {
            base_spec: m1,
            finer_specs: vec![m1],
        };
        let coverage_cache = CoverageCache::default();
        let key = derived_key(h1);
        let gap = compute_gap(
            &store,
            &coverage_cache,
            &candidates,
            &key,
            two_hours,
            Anchor::UTC,
        )
        .unwrap();
        assert_eq!(gap.covered, vec![(two_hours, Some(m1))]);
        assert_eq!(gap.missing, Vec::new());

        let bar_cache = BarCache::new(usize::MAX);
        let generations = GenerationTracker::default();
        let bars = materialize(
            &store,
            &bar_cache,
            &generations,
            &key,
            Anchor::UTC,
            two_hours,
            &gap,
        )
        .unwrap();

        assert_eq!(
            bars.len(),
            1,
            "only the complete first hour may be emitted; the second (missing minute 30) must not be"
        );
        assert_eq!(bars[0].ts_open, UnixNanos::from_secs(0).unwrap());
    }

    #[test]
    fn compute_gap_of_an_uncovered_derived_request_reports_the_whole_range_missing_at_base_spec() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let m1 = BarSpec::new(1, BarUnit::Minute);
        let h1 = BarSpec::new(1, BarUnit::Hour);
        let candidates = Candidates {
            base_spec: m1,
            finer_specs: vec![m1],
        };
        let coverage_cache = CoverageCache::default();
        let key = derived_key(h1);
        let range = secs_range(0, 3600);
        let gap = compute_gap(
            &store,
            &coverage_cache,
            &candidates,
            &key,
            range,
            Anchor::UTC,
        )
        .unwrap();
        assert_eq!(gap.covered, Vec::new());
        assert_eq!(gap.missing, vec![range]);
    }

    /// Required test: "stitch adjacent stored
    /// specs." Hour 0 is stored only at M1, hour 1 only at M15 — neither
    /// candidate covers the whole two-hour request, so the pre-M8.5 ladder
    /// would have rejected both and reported the entire range missing even
    /// though every minute of it is already on disk. This test proves two
    /// things at once: the stitched result actually resolves both hours,
    /// and — the plan's own instruction — that it produces *exactly* the
    /// same bars a non-stitching request for each hour individually would,
    /// never something a single-candidate resolution could not also have
    /// produced.
    #[test]
    fn stitching_two_adjacent_fully_covered_hours_matches_resolving_them_individually() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let m1 = BarSpec::new(1, BarUnit::Minute);
        let m15 = BarSpec::new(15, BarUnit::Minute);
        let h1 = BarSpec::new(1, BarUnit::Hour);
        let hour0 = secs_range(0, 3600);
        let hour1 = secs_range(3600, 7200);
        let key = derived_key(h1);

        let hour0_bars: Vec<Bar> = (0..60).map(|i| bar(i * 60, 1, 1)).collect();
        store
            .write(
                &super::venue_key(&key, m1),
                Anchor::UTC,
                0,
                0,
                hour0,
                &hour0_bars,
            )
            .unwrap();
        let hour1_bars: Vec<Bar> = (0..4).map(|i| bar(3600 + i * 900, 2, 1)).collect();
        store
            .write(
                &super::venue_key(&key, m15),
                Anchor::UTC,
                0,
                0,
                hour1,
                &hour1_bars,
            )
            .unwrap();

        let candidates = Candidates {
            base_spec: m1,
            finer_specs: vec![m1, m15],
        };
        let coverage_cache = CoverageCache::default();
        let bar_cache = BarCache::new(usize::MAX);
        let generations = GenerationTracker::default();

        // The stitched, whole-range request. `compute_gap` is called
        // directly (rather than through `resolve`) since the assertions
        // below need the plan itself, not just the bars it produces.
        let two_hours = secs_range(0, 7200);
        let stitched_gap = compute_gap(
            &store,
            &coverage_cache,
            &candidates,
            &key,
            two_hours,
            Anchor::UTC,
        )
        .unwrap();
        assert_eq!(
            stitched_gap.missing,
            Vec::new(),
            "both hours are individually fully covered, just by different specs"
        );
        assert_eq!(
            stitched_gap.covered,
            vec![(hour0, Some(m1)), (hour1, Some(m15))]
        );
        let stitched_bars = materialize(
            &store,
            &bar_cache,
            &generations,
            &key,
            Anchor::UTC,
            two_hours,
            &stitched_gap,
        )
        .unwrap();

        // The non-stitching baseline: resolve each hour on its own, exactly
        // as its single-candidate ladder always could.
        let mut expected = resolve(
            &store,
            &coverage_cache,
            &candidates,
            &bar_cache,
            &generations,
            &key,
            hour0,
        );
        expected.extend(resolve(
            &store,
            &coverage_cache,
            &candidates,
            &bar_cache,
            &generations,
            &key,
            hour1,
        ));
        assert_eq!(
            stitched_bars, expected,
            "stitching must never produce a bar a non-stitching, per-hour request would not"
        );
    }

    /// Required test: the completeness rule stays
    /// absolute under stitching. M15 covers only 45 of hour 1's 60 minutes
    ///   — a genuine partial-bucket sliver, not a whole adjacent region — and
    /// crediting it as "resolved" would silently strand that hour forever
    /// (dropped from `missing`, so `ensure()` never fetches it, while the
    /// aggregator would still correctly refuse to emit a bar for it). The
    /// whole hour must instead stay `missing`.
    #[test]
    fn a_candidates_partial_bucket_coverage_is_never_credited_as_resolved() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let m1 = BarSpec::new(1, BarUnit::Minute);
        let m15 = BarSpec::new(15, BarUnit::Minute);
        let h1 = BarSpec::new(1, BarUnit::Hour);
        let key = derived_key(h1);
        let hour1 = secs_range(3600, 7200);
        let partial = secs_range(3600, 6300); // the first 45 of 60 minutes

        let m15_bars: Vec<Bar> = (0..3).map(|i| bar(3600 + i * 900, 2, 1)).collect();
        store
            .write(
                &super::venue_key(&key, m15),
                Anchor::UTC,
                0,
                0,
                partial,
                &m15_bars,
            )
            .unwrap();

        let candidates = Candidates {
            base_spec: m1,
            finer_specs: vec![m1, m15],
        };
        let coverage_cache = CoverageCache::default();
        let gap = compute_gap(
            &store,
            &coverage_cache,
            &candidates,
            &key,
            hour1,
            Anchor::UTC,
        )
        .unwrap();

        assert_eq!(
            gap.covered,
            Vec::new(),
            "45 of 60 minutes cannot complete an H1 bucket; nothing may be credited"
        );
        assert_eq!(
            gap.missing,
            vec![hour1],
            "the whole hour must stay missing, not be silently stranded as neither covered nor fetchable"
        );
    }
}
