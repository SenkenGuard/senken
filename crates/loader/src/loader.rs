//! [`SeriesLoader`] — the resolution ladder, cache, single-flight fetching
//! and job scheduling tying every other module in this crate together
//! .

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use senken_core::{TimeRange, UnixNanos};
use senken_series::{Anchor, Bar, BarSpec, Clock, Origin, SeriesKey};
use senken_store::Store;
use tokio::sync::{oneshot, watch};

use crate::cache::BarCache;
use crate::chunk::{ChunkKey, ChunkSingleFlight, split_into_chunks};
use crate::coverage::CoverageCache;
use crate::error::LoadError;
use crate::generation::GenerationTracker;
use crate::job::{JobHandle, JobId, JobOutcome, JobSnapshot, Phase, Priority, Requirement};
use crate::ladder::{self, Candidates};
use crate::priority_gate::PriorityGate;
use crate::source::BarSource;

/// The default byte budget for this loader's internal bar cache — 64 MiB. An
/// explicit, overridable default ("an explicit setting ... not
/// a buried constant"), sized generously against the arithmetic in design
/// D16 (a full year of M1 for one symbol decodes to roughly 25 MB; a chart
/// viewport is a few hundred KB), not a measurement of this deployment.
pub const DEFAULT_CACHE_BYTES: usize = 64 * 1024 * 1024;

/// The default ceiling on chunk fetches running at once, across every job
/// on one loader. Deliberately conservative and undocumented by any venue,
/// the same posture `senken-venue`'s own `DEFAULT_MAX_CONCURRENT` takes for
/// the same reason: this loader has no per-venue quota to reason from —
/// that lives in whatever `BarSource` implementation this is paired with —
/// so it caps its own fan-out modestly instead of assuming one.
pub const DEFAULT_MAX_CONCURRENT_FETCHES: usize = 4;

/// The largest number of retry attempts [`SeriesLoader`] makes for one
/// chunk before giving up and failing the job. A secondary safety net, not
/// the primary retry mechanism: a real `BarSource` sits on top of
/// `senken-venue`'s own retry/backoff and should rarely exhaust
/// this on top of that.
const MAX_CHUNK_ATTEMPTS: u32 = 5;

/// The first retry backoff, doubling per attempt (matching the convention
/// `senken-venue::fetch_bytes` already uses) up to a cap.
const FIRST_BACKOFF_NANOS: i64 = 250_000_000;

/// How often [`SeriesLoader::subscribe`] is refreshed while a job is
/// otherwise silently making progress ("coalesces ... emit on
/// phase change and on a bounded interval"). [`SeriesLoader::jobs`]/
/// [`SeriesLoader::job`] are unaffected by this — they always read each
/// job's live snapshot directly, never the throttled broadcast.
const SUBSCRIBE_COALESCE_NANOS: i64 = 100_000_000;

/// Builds a [`SeriesLoader`]. Required parameters go through
/// [`Self::new`]; everything with a sensible default is a `with_*` method.
pub struct SeriesLoaderBuilder {
    store: Store,
    source: Arc<dyn BarSource>,
    clock: Arc<dyn Clock>,
    base_spec: BarSpec,
    finer_specs: Vec<BarSpec>,
    cache_bytes: usize,
    max_concurrent_fetches: usize,
}

impl SeriesLoaderBuilder {
    /// Starts a builder. `base_spec` is the finest spec this loader ever
    /// fetches directly from `source` — the canonical base every
    /// [`senken_series::Origin::Derived`] request is ultimately folded
    /// from ("M1 keeps its special status ... the only spec
    /// a tick-capable source needs").
    #[must_use]
    pub fn new(
        store: Store,
        source: Arc<dyn BarSource>,
        clock: Arc<dyn Clock>,
        base_spec: BarSpec,
    ) -> Self {
        Self {
            store,
            source,
            clock,
            base_spec,
            finer_specs: Vec::new(),
            cache_bytes: DEFAULT_CACHE_BYTES,
            max_concurrent_fetches: DEFAULT_MAX_CONCURRENT_FETCHES,
        }
    }

    /// Specs the ladder should look for already stored (`Origin::Venue`)
    /// before falling back to fetching `base_spec` (step 3).
    /// `base_spec` itself is always implicitly a candidate; it does not
    /// need to be repeated here.
    #[must_use]
    pub fn finer_specs(mut self, specs: Vec<BarSpec>) -> Self {
        self.finer_specs = specs;
        self
    }

    /// Overrides [`DEFAULT_CACHE_BYTES`].
    #[must_use]
    pub fn cache_bytes(mut self, bytes: usize) -> Self {
        self.cache_bytes = bytes;
        self
    }

    /// Overrides [`DEFAULT_MAX_CONCURRENT_FETCHES`].
    #[must_use]
    pub fn max_concurrent_fetches(mut self, n: usize) -> Self {
        self.max_concurrent_fetches = n;
        self
    }

    /// Finishes building.
    ///
    /// # Errors
    /// [`LoadError::Aggregate`]... actually never — see `# Panics`.
    ///
    /// # Panics
    /// If `base_spec` has no fixed duration (only
    /// [`senken_series::BarUnit::Month`] does not) — chunk sizing needs a
    /// fixed span, and a `Month`-unit base spec makes no sense as
    /// something fetched in venue-page-sized pieces.
    #[must_use]
    pub fn build(self) -> SeriesLoader {
        assert!(
            self.base_spec.duration_nanos().is_some(),
            "SeriesLoader's base_spec must have a fixed duration"
        );
        let (jobs_tx, _jobs_rx) = watch::channel(Vec::new());
        SeriesLoader {
            inner: Arc::new(Inner {
                store: self.store,
                source: self.source,
                clock: self.clock,
                candidates: Candidates {
                    base_spec: self.base_spec,
                    finer_specs: self.finer_specs,
                },
                bar_cache: BarCache::new(self.cache_bytes),
                coverage_cache: CoverageCache::default(),
                generations: GenerationTracker::default(),
                inflight: ChunkSingleFlight::default(),
                fetch_gate: PriorityGate::new(self.max_concurrent_fetches),
                jobs: Mutex::new(HashMap::new()),
                next_job_id: AtomicU64::new(0),
                jobs_tx,
                last_broadcast: Mutex::new(None),
                throughput: Mutex::new((0, 0)),
            }),
        }
    }
}

/// How long a job stays queryable after it stops running.
///
/// Not zero, and that matters: `GET /api/bars/jobs/{id}` is a poll, so a
/// caller that started a job has to be able to see it reach
/// [`Phase::Done`] at least once. Dropping the record the instant the work
/// ended would answer "no such job" — which, to that caller, is
/// indistinguishable from an id it made up. Past this window nobody is
/// waiting on the answer any more, and keeping it is only a slower memory
/// leak.
const FINISHED_JOB_RETENTION_NANOS: i64 = 60 * 1_000_000_000;

/// Drops every job that finished more than [`FINISHED_JOB_RETENTION_NANOS`]
/// ago. Running jobs are never touched, whatever their age — a long
/// backfill is not stale, it is busy.
///
/// A free function over the map so the retention rule can be tested against
/// a table built by hand, without driving real fetches to produce finished
/// jobs first.
fn retire_finished(jobs: &mut HashMap<JobId, Arc<JobRecord>>, now: senken_core::UnixNanos) {
    jobs.retain(|_, record| {
        let finished = *record
            .finished_at
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(finished) = finished else {
            return true;
        };
        now.as_nanos().saturating_sub(finished.as_nanos()) < FINISHED_JOB_RETENTION_NANOS
    });
}

struct JobRecord {
    snapshot: Mutex<JobSnapshot>,
    cancelled: AtomicBool,
    /// When this job stopped running, per this loader's [`Clock`] — `None`
    /// while it is still going. Kept on the record rather than in
    /// [`JobSnapshot`] because it answers a question about the *record*
    /// (how long to keep it), not about the job's progress, which is what
    /// the snapshot reports.
    finished_at: Mutex<Option<senken_core::UnixNanos>>,
}

/// `price_scale`/`qty_scale`/`priority`, bundled purely to keep `run_job`
/// and `process_chunk` under the workspace's `too_many_arguments` lint —
/// all three are fixed for a job's entire lifetime, so passing them as one
/// value costs nothing beyond the three separate ones this replaces.
#[derive(Clone, Copy)]
struct ChunkParams {
    price_scale: u8,
    qty_scale: u8,
    priority: Priority,
}

struct Inner {
    store: Store,
    source: Arc<dyn BarSource>,
    clock: Arc<dyn Clock>,
    candidates: Candidates,
    bar_cache: BarCache,
    coverage_cache: CoverageCache,
    generations: GenerationTracker,
    inflight: ChunkSingleFlight,
    /// Caps concurrent chunk fetches across every job, servicing
    /// a freed slot by [`Priority`] rather than arrival order — see [`PriorityGate`]'s own module docs.
    fetch_gate: PriorityGate,
    jobs: Mutex<HashMap<JobId, Arc<JobRecord>>>,
    next_job_id: AtomicU64,
    jobs_tx: watch::Sender<Vec<JobSnapshot>>,
    /// `None` until the first broadcast — the very first publish always
    /// goes through regardless of the coalescing interval.
    last_broadcast: Mutex<Option<senken_core::UnixNanos>>,
    /// `(sum of nanoseconds spent per completed chunk, count)` — the
    /// measured throughput [`JobSnapshot::estimate`]/[`Requirement::estimate`]
    /// are derived from. Never a guess ("an honest `None` is
    /// better than a wrong number").
    throughput: Mutex<(u64, u64)>,
}

/// The resolution ladder, cache, single-flight fetching and job scheduling
/// between a chart and [`senken_store::Store`].
///
/// Cheap to clone — an `Arc` around everything — so a spawned job task can
/// hold its own handle independently of the caller that started it.
#[derive(Clone)]
pub struct SeriesLoader {
    inner: Arc<Inner>,
}

/// What [`SeriesLoader::resolve`] found: whatever bars are already
/// resolvable right now, plus what is still missing. Not
/// part of the plan's own sketch by name, but is exactly what that prose
/// describes a caller needing back; [`SeriesLoader::ensure`] is what fills
/// `missing` in the background, observable via [`SeriesLoader::jobs`].
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Bars already available, ascending by `ts_open`.
    pub bars: Vec<Bar>,
    /// Ranges not yet resolvable — call [`SeriesLoader::ensure`] to start
    /// filling them.
    pub missing: Vec<TimeRange>,
}

impl SeriesLoader {
    /// Pure inspection: touches no network, starts no work,
    /// mutates nothing beyond this loader's own coverage cache (a
    /// read-through cache over [`senken_store::Store::coverage`], itself a
    /// directory listing — no file is ever opened here).
    ///
    /// # Errors
    /// [`LoadError::Store`] if a coverage directory exists but cannot be
    /// listed.
    pub fn plan(
        &self,
        key: &SeriesKey,
        range: TimeRange,
        anchor: Anchor,
    ) -> Result<Requirement, LoadError> {
        let gap = ladder::compute_gap(
            &self.inner.store,
            &self.inner.coverage_cache,
            &self.inner.candidates,
            key,
            range,
            anchor,
        )?;
        let fetch_span = self.chunk_span_nanos(self.fetch_spec_for(key));
        let mut chunks: u32 = 0;
        let mut estimated_bars: u64 = 0;
        let fetch_duration = self.fetch_spec_for(key).duration_nanos().unwrap_or(1);
        for g in &gap.missing {
            let span = g.end().as_nanos() - g.start().as_nanos();
            chunks += chunk_count(span, fetch_span);
            estimated_bars += u64::try_from(span / fetch_duration.max(1)).unwrap_or(0);
        }
        Ok(Requirement {
            key: key.clone(),
            range,
            // `Requirement.covered` is plain ranges (the // sketch): which candidate spec backs each stitched piece
            // is this ladder's own internal bookkeeping, not
            // something a caller inspecting "what is missing" needs.
            covered: gap.covered.iter().map(|(r, _)| *r).collect(),
            missing: gap.missing,
            chunks,
            estimated_bars,
            estimate: self.estimate_for(chunks),
        })
    }

    /// Returns whatever is already resolvable for `key`/`range` right now
    /// (the "progressive delivery": never blocks on a fetch).
    /// Fills [`Resolved::missing`] with anything that is not; call
    /// [`Self::ensure`] to start fetching it.
    ///
    /// Runs the actual Parquet decode and aggregation on a blocking pool
    ///, never on the calling async task.
    ///
    /// # Errors
    /// [`LoadError::Store`]/[`LoadError::Aggregate`] as reading or folding
    /// stored bars fails.
    pub async fn resolve(
        &self,
        key: &SeriesKey,
        range: TimeRange,
        anchor: Anchor,
    ) -> Result<Resolved, LoadError> {
        if let Some(cached) = self
            .inner
            .bar_cache
            .get(key, range, &self.inner.generations)
        {
            return Ok(Resolved {
                bars: cached.bars.to_vec(),
                missing: Vec::new(),
            });
        }
        let inner = Arc::clone(&self.inner);
        let key = key.clone();
        run_blocking(move || {
            let gap = ladder::compute_gap(
                &inner.store,
                &inner.coverage_cache,
                &inner.candidates,
                &key,
                range,
                anchor,
            )?;
            let bars = ladder::materialize(
                &inner.store,
                &inner.bar_cache,
                &inner.generations,
                &key,
                anchor,
                range,
                &gap,
            )?;
            Ok(Resolved {
                bars,
                missing: gap.missing,
            })
        })
        .await
    }

    /// Enqueues the fetches [`Self::plan`] would report as missing for
    /// `key`/`range`, and returns immediately with a handle to watch —
    /// never a future that resolves only once a multi-minute backfill
    /// finishes.
    ///
    /// `price_scale`/`qty_scale` are supplied here, not derived: they are
    /// a property of the specific instrument, and this crate
    /// has no instrument-catalog dependency to source them from
    /// automatically — wiring that up is `senken-runtime`'s job (plan Part
    /// C2), exactly as [`senken_store::Store::write`] itself already
    /// requires them as explicit per-call parameters rather than deriving
    /// them from a `Bar`. This — along with the explicit `anchor` — is a
    /// deliberate widening of the plan's illustrative `ensure(&self, key,
    /// range, priority)` signature, for the same reason `senken-store`'s
    /// M5 executor widened `coverage`/`read_range` beyond their own
    /// sketch: an unavoidable consequence of a detail the plan's sketch
    /// left implicit.
    pub fn ensure(
        &self,
        key: &SeriesKey,
        range: TimeRange,
        anchor: Anchor,
        price_scale: u8,
        qty_scale: u8,
        priority: Priority,
    ) -> JobHandle {
        let id = JobId(self.inner.next_job_id.fetch_add(1, Ordering::SeqCst));
        let record = Arc::new(JobRecord {
            snapshot: Mutex::new(JobSnapshot {
                id,
                key: key.clone(),
                range,
                phase: Phase::Queued,
                chunks_total: 0,
                chunks_done: 0,
                bars_written: 0,
                started_at: None,
                estimate: None,
                last_error: None,
                priority,
            }),
            cancelled: AtomicBool::new(false),
            finished_at: Mutex::new(None),
        });
        {
            let mut jobs = self
                .inner
                .jobs
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            // The only place this map can grow, and therefore the place to
            // shrink it. Before this, a job record was inserted and never
            // removed — one per `ensure()`, which a chart calls on every
            // scroll, prefetch and closed bar, for the life of the process.
            retire_finished(&mut jobs, self.inner.clock.now());
            jobs.insert(id, Arc::clone(&record));
        }
        self.publish_jobs(true);

        let (tx, rx) = oneshot::channel();
        let loader = self.clone();
        let key = key.clone();
        tokio::spawn(async move {
            let outcome = loader
                .run_job(
                    &record,
                    &key,
                    range,
                    anchor,
                    ChunkParams {
                        price_scale,
                        qty_scale,
                        priority,
                    },
                )
                .await;
            {
                let mut snap = record
                    .snapshot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                snap.phase = Phase::Done;
            }
            *record
                .finished_at
                .lock()
                .unwrap_or_else(PoisonError::into_inner) = Some(loader.inner.clock.now());
            loader.publish_jobs(true);
            let _ = tx.send(outcome);
        });

        JobHandle { id, outcome: rx }
    }

    /// Every job this loader currently knows about, live.
    #[must_use]
    pub fn jobs(&self) -> Vec<JobSnapshot> {
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .map(|record| {
                record
                    .snapshot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            })
            .collect()
    }

    /// One job's live snapshot, if it exists.
    #[must_use]
    pub fn job(&self, id: JobId) -> Option<JobSnapshot> {
        self.inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .map(|record| {
                record
                    .snapshot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone()
            })
    }

    /// Whether this loader's source can fetch `spec` directly.
    ///
    /// This reports the source capability captured when the loader was
    /// built. Callers use it before enqueueing an explicit native download,
    /// so an unsupported interval is rejected rather than becoming a job
    /// that can only fail after reaching the venue.
    #[must_use]
    pub fn supports_venue_spec(&self, spec: BarSpec) -> bool {
        self.inner.candidates.base_spec == spec || self.inner.candidates.finer_specs.contains(&spec)
    }

    /// A live, coalesced feed of every job's snapshot.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<Vec<JobSnapshot>> {
        self.inner.jobs_tx.subscribe()
    }

    /// How many chunk fetches this loader has actually *run*, as opposed
    /// to how many times a job *asked* for one — the single-flight
    /// mechanism collapses duplicate concurrent
    /// requests for the same chunk into one, and this is how a caller (or
    /// a test) can observe that it actually happened rather than merely
    /// trusting it.
    #[must_use]
    pub fn single_flight_fetch_starts(&self) -> u64 {
        self.inner.inflight.fetch_starts()
    }

    /// How many chunk fetches are currently queued behind this loader's
    /// concurrency ceiling, waiting for a slot. Test-only: lets
    /// a test synchronise on a known contention state deterministically
    /// instead of guessing a sleep duration.
    #[cfg(test)]
    pub(crate) fn fetch_gate_waiting_count(&self) -> usize {
        self.inner.fetch_gate.waiting_count()
    }

    /// This loader's bar-cache metrics ("an explicit setting
    /// with metrics ... not a buried constant") — bytes cached, hit rate,
    /// evictions.
    #[must_use]
    pub fn cache_metrics(&self) -> crate::cache::CacheMetrics {
        self.inner.bar_cache.metrics()
    }

    /// The observed left edge for the venue series that backs `key`. A
    /// derived request intentionally asks the same underlying venue-spec
    /// fact: aggregation cannot manufacture older bars, but the fact still
    /// remains per fetch spec rather than being shared across every spec.
    ///
    /// # Errors
    /// Returns [`LoadError::Store`] if the persisted boundary cannot be
    /// read.
    pub fn earliest_available(
        &self,
        key: &SeriesKey,
        anchor: Anchor,
    ) -> Result<Option<UnixNanos>, LoadError> {
        let fetch_spec = self.fetch_spec_for(key);
        let fetch_key = match key.origin {
            Origin::Venue => key.clone(),
            Origin::Derived => ladder::venue_key(key, fetch_spec),
        };
        Ok(self.inner.store.earliest_available(&fetch_key, anchor)?)
    }

    /// Requests cancellation of job `id`. Takes effect before the job's
    /// *next* chunk starts — a chunk already being fetched or written
    /// completes and is kept ("anything already written is
    /// kept, since it is still true"). A no-op if `id` does not exist or
    /// has already finished.
    pub fn cancel(&self, id: JobId) {
        if let Some(record) = self
            .inner
            .jobs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
        {
            record.cancelled.store(true, Ordering::SeqCst);
        }
    }

    fn fetch_spec_for(&self, key: &SeriesKey) -> BarSpec {
        match key.origin {
            Origin::Venue => key.spec,
            Origin::Derived => self.inner.candidates.fetch_spec_for(key.spec),
        }
    }

    fn chunk_span_nanos(&self, fetch_spec: BarSpec) -> i64 {
        let duration = fetch_spec.duration_nanos().unwrap_or(1).max(1);
        let max_rows = i64::try_from(self.inner.source.max_rows().max(1)).unwrap_or(i64::MAX);
        duration.saturating_mul(max_rows)
    }

    fn estimate_for(&self, chunks_remaining: u32) -> Option<Duration> {
        let (sum, count) = *self
            .inner
            .throughput
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if count == 0 {
            return None;
        }
        let avg_nanos_per_chunk = sum / count;
        Some(Duration::from_nanos(
            avg_nanos_per_chunk.saturating_mul(u64::from(chunks_remaining)),
        ))
    }

    fn record_throughput(&self, nanos: u64) {
        let mut state = self
            .inner
            .throughput
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        state.0 = state.0.saturating_add(nanos.max(1));
        state.1 = state.1.saturating_add(1);
    }

    /// Broadcasts the current set of job snapshots to [`Self::subscribe`]'s
    /// receivers. `force` bypasses the coalescing interval — used for the
    /// job's very first publish and its terminal one, so a subscriber
    /// never misses either end of a job's life even if every progress tick
    /// in between was throttled.
    fn publish_jobs(&self, force: bool) {
        let now = self.inner.clock.now();
        {
            let mut last = self
                .inner
                .last_broadcast
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !force
                && let Some(last_time) = *last
                && now.as_nanos() - last_time.as_nanos() < SUBSCRIBE_COALESCE_NANOS
            {
                return;
            }
            *last = Some(now);
        }
        let snapshot = self.jobs();
        let _ = self.inner.jobs_tx.send(snapshot);
    }

    /// Pure-inspection gap computation, run on the blocking pool
    ///  since it touches the filesystem via `Store::coverage`.
    async fn compute_gap_blocking(
        &self,
        key: &SeriesKey,
        range: TimeRange,
        anchor: Anchor,
    ) -> Result<ladder::GapPlan, LoadError> {
        let inner = Arc::clone(&self.inner);
        let key = key.clone();
        run_blocking(move || {
            ladder::compute_gap(
                &inner.store,
                &inner.coverage_cache,
                &inner.candidates,
                &key,
                range,
                anchor,
            )
        })
        .await
    }

    async fn run_job(
        &self,
        record: &Arc<JobRecord>,
        key: &SeriesKey,
        range: TimeRange,
        anchor: Anchor,
        params: ChunkParams,
    ) -> JobOutcome {
        let gap = match self.compute_gap_blocking(key, range, anchor).await {
            Ok(gap) => gap,
            Err(error) => return JobOutcome::Failed(error),
        };

        let fetch_spec = self.fetch_spec_for(key);
        let fetch_key = match key.origin {
            Origin::Venue => key.clone(),
            Origin::Derived => ladder::venue_key(key, fetch_spec),
        };
        let chunk_span = self.chunk_span_nanos(fetch_spec);
        let chunks: Vec<TimeRange> = gap
            .missing
            .iter()
            .flat_map(|g| split_into_chunks(*g, chunk_span))
            .collect();

        {
            let mut snap = record
                .snapshot
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            snap.phase = Phase::Downloading;
            snap.chunks_total = u32::try_from(chunks.len()).unwrap_or(u32::MAX);
            snap.started_at = Some(self.inner.clock.now());
        }
        self.publish_jobs(true);

        for chunk_range in chunks {
            if record.cancelled.load(Ordering::SeqCst) {
                return JobOutcome::Cancelled;
            }
            if let Err(outcome) = self
                .process_chunk(record, &fetch_key, fetch_spec, anchor, chunk_range, params)
                .await
            {
                return outcome;
            }
        }

        if key.origin == Origin::Derived {
            {
                let mut snap = record
                    .snapshot
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner);
                snap.phase = Phase::Aggregating;
            }
            self.publish_jobs(true);
            // Best-effort cache warm — a failure here does not fail the
            // job, since every chunk this job promised is already safely
            // written; a caller can always call `resolve` again.
            if let Err(error) = self.resolve(key, range, anchor).await {
                tracing::debug!(%error, "post-fetch cache warm failed; chunks are still written");
            }
        }

        JobOutcome::Completed
    }

    /// Fetches, writes and accounts for exactly one chunk. `Err(outcome)`
    /// is the terminal [`JobOutcome`] `run_job` should return immediately;
    /// `Ok(())` means keep going to the next chunk. Split out of `run_job`
    /// purely to keep that function's length reasonable — this is not
    /// meant to be called from anywhere else.
    async fn process_chunk(
        &self,
        record: &Arc<JobRecord>,
        fetch_key: &SeriesKey,
        fetch_spec: BarSpec,
        anchor: Anchor,
        chunk_range: TimeRange,
        params: ChunkParams,
    ) -> Result<(), JobOutcome> {
        let ChunkParams {
            price_scale,
            qty_scale,
            priority,
        } = params;
        // Waits for a slot, ranked against every other job currently
        // contending for one — a `Visible` job's
        // chunk overtakes a `Background` job's queued one here, not merely
        // in the reported `Priority` field.
        let _permit = self.inner.fetch_gate.acquire(priority).await;

        let started = self.inner.clock.now();
        let bars = self
            .fetch_chunk_with_retry(record, fetch_key, fetch_spec, chunk_range)
            .await
            .map_err(|error| JobOutcome::Failed(LoadError::Fetch(error)))?;

        {
            let mut snap = record
                .snapshot
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            snap.phase = Phase::Writing;
        }
        self.publish_jobs(false);

        // A venue that answers with nothing has told us something true, and
        // there is nothing to write: coverage is derived from the names of
        // the files on disk, so an empty file would claim a range it does not
        // hold. Failing the job here instead — which is what writing an empty
        // batch does — would turn "the venue has no bars for this window"
        // into an error the reader cannot act on, and would skip the boundary
        // update below that is the only way the left edge is ever learnt.
        let write_result = if bars.is_empty() {
            Ok(())
        } else {
            let inner = Arc::clone(&self.inner);
            let fetch_key = fetch_key.clone();
            let bars = Arc::clone(&bars);
            run_blocking(move || {
                inner
                    .store
                    .write(
                        &fetch_key,
                        anchor,
                        price_scale,
                        qty_scale,
                        chunk_range,
                        &bars,
                    )
                    .map_err(LoadError::from)
            })
            .await
        };
        if let Err(error) = write_result {
            let mut snap = record
                .snapshot
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            snap.last_error = Some(error.to_string());
            drop(snap);
            self.publish_jobs(true);
            return Err(JobOutcome::Failed(error));
        }

        if let Err(error) = self
            .update_earliest_boundary(fetch_key, anchor, fetch_spec, chunk_range, &bars)
            .await
        {
            let mut snap = record
                .snapshot
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            snap.last_error = Some(error.to_string());
            drop(snap);
            self.publish_jobs(true);
            return Err(JobOutcome::Failed(error));
        }
        self.inner.coverage_cache.invalidate(fetch_key);
        self.inner.bar_cache.invalidate_key(fetch_key);
        self.inner.generations.bump(fetch_key);

        let elapsed = (self.inner.clock.now().as_nanos() - started.as_nanos()).max(1);
        self.record_throughput(u64::try_from(elapsed).unwrap_or(1));

        {
            let mut snap = record
                .snapshot
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            snap.chunks_done += 1;
            snap.bars_written = snap.bars_written.saturating_add(bars.len() as u64);
            snap.phase = Phase::Downloading;
            snap.last_error = None;
            snap.estimate = self.estimate_for(snap.chunks_total - snap.chunks_done);
        }
        self.publish_jobs(false);
        Ok(())
    }

    /// Records the venue's left edge, or removes a recorded edge once a
    /// later response reaches earlier than it.
    ///
    /// A chunk is evidence that the venue has nothing older **only when it
    /// asked for a whole page and came back completely empty**. Two weaker
    /// signals look reasonable and are both wrong:
    ///
    /// - A chunk narrower than a page cannot return a page's worth of rows,
    ///   so counting its rows proves nothing. `split_into_chunks` leaves a
    ///   short trailing chunk on any span that is not a whole multiple of the
    ///   page, so this fires on ordinary requests.
    /// - A whole page that returns *some* rows is ambiguous: one minute with
    ///   no trades produces exactly the same short response as the true start
    ///   of history.
    ///
    /// The asymmetry settles it. Missing the edge costs one wasted request
    /// that the next page corrects. Claiming it wrongly makes every older bar
    /// unreachable, because the client stops asking — and then nothing
    /// arrives to revoke the claim.
    ///
    /// Deliberately after the Parquet write: a failed write must never make a
    /// future reader believe the historical probe succeeded.
    async fn update_earliest_boundary(
        &self,
        fetch_key: &SeriesKey,
        anchor: Anchor,
        fetch_spec: BarSpec,
        chunk_range: TimeRange,
        bars: &[Bar],
    ) -> Result<(), LoadError> {
        let span = chunk_range
            .end()
            .as_nanos()
            .saturating_sub(chunk_range.start().as_nanos());
        let whole_page = span >= self.chunk_span_nanos(fetch_spec);
        let exhausted = whole_page && bars.is_empty();
        let reached = bars.first().map(|bar| bar.ts_open);
        let observed_earliest = chunk_range.end();

        let inner = Arc::clone(&self.inner);
        let fetch_key = fetch_key.clone();
        run_blocking(move || {
            let existing = inner.store.earliest_available(&fetch_key, anchor)?;
            if exhausted {
                match existing {
                    Some(previous) if previous <= observed_earliest => Ok(()),
                    _ => inner.store.record_earliest_available(
                        &fetch_key,
                        anchor,
                        Some(observed_earliest),
                    ),
                }
            } else if reached.is_some_and(|first| existing.is_some_and(|prev| first < prev)) {
                // A real bar older than the recorded edge disproves it.
                inner
                    .store
                    .record_earliest_available(&fetch_key, anchor, None)
            } else {
                Ok(())
            }
        })
        .await
        .map_err(LoadError::from)
    }

    async fn fetch_chunk_with_retry(
        &self,
        record: &Arc<JobRecord>,
        fetch_key: &SeriesKey,
        fetch_spec: BarSpec,
        chunk_range: TimeRange,
    ) -> Result<Arc<[Bar]>, crate::source::FetchError> {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            let chunk_key = ChunkKey {
                source_id: fetch_key.source_id.clone(),
                symbol: fetch_key.symbol.clone(),
                fetch_spec,
                chunk_range,
            };
            let source = Arc::clone(&self.inner.source);
            let symbol = fetch_key.symbol.to_string();
            let result = self
                .inner
                .inflight
                .fetch_or_join(chunk_key, move || async move {
                    source.bars(&symbol, fetch_spec, chunk_range).await
                })
                .await;

            match result {
                Ok(bars) => return Ok(bars),
                Err(error) if error.is_retryable() && attempt < MAX_CHUNK_ATTEMPTS => {
                    {
                        let mut snap = record
                            .snapshot
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        snap.last_error = Some(error.to_string());
                    }
                    self.publish_jobs(true);
                    let backoff_nanos =
                        FIRST_BACKOFF_NANOS.saturating_mul(1i64 << (attempt - 1).min(10));
                    let now = self.inner.clock.now();
                    let wake_at = now
                        .checked_add(Duration::from_nanos(
                            u64::try_from(backoff_nanos).unwrap_or(u64::MAX),
                        ))
                        .unwrap_or(now);
                    self.inner.clock.sleep_until(wake_at).await;
                }
                Err(error) => return Err(error),
            }
        }
    }
}

/// How many `max_span_nanos`-wide chunks `span_nanos` splits into (ceiling
/// division) — the same arithmetic [`split_into_chunks`] performs, kept
/// separate so [`SeriesLoader::plan`] can report a chunk *count* without
/// materialising the [`TimeRange`]s themselves.
fn chunk_count(span_nanos: i64, max_span_nanos: i64) -> u32 {
    if span_nanos <= 0 {
        return 0;
    }
    let max_span_nanos = max_span_nanos.max(1);
    // `i64::div_ceil` is unstable for signed integers on this toolchain;
    // both operands are positive here (checked above), so plain ceiling
    // division is safe.
    let chunks = (span_nanos + max_span_nanos - 1) / max_span_nanos;
    u32::try_from(chunks).unwrap_or(u32::MAX)
}

/// Runs `f` on the blocking pool (Parquet decode and
/// aggregation must never run on an async worker thread), matching the
/// `run_blocking` helper `senken-marketdata`'s registry already uses for
/// the same reason.
async fn run_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(f).await {
        Ok(value) => value,
        Err(error) => match error.try_into_panic() {
            Ok(payload) => std::panic::resume_unwind(payload),
            Err(error) => panic!("blocking loader task cancelled: {error}"),
        },
    }
}

#[cfg(test)]
mod job_retention {
    // Every `ensure()` used to insert a job record that was never removed —
    // and a chart calls `ensure()` on every scroll, prefetch and closed bar.
    // What matters here is that the table *shrinks*, which no caller of this
    // crate can observe, so these exercise the retention rule directly.
    use super::{FINISHED_JOB_RETENTION_NANOS, JobId, JobRecord, Phase, Priority, retire_finished};
    use crate::job::JobSnapshot;
    use senken_core::{TimeRange, UnixNanos};
    use senken_series::{BarSpec, BarUnit, SeriesKey};
    use std::collections::HashMap;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    fn key() -> SeriesKey {
        SeriesKey::new(
            "test-venue",
            "BTCUSDT",
            senken_series::Origin::Venue,
            BarSpec::new(1, BarUnit::Minute),
        )
    }

    /// A record in whatever state the test needs: still running (`None`) or
    /// finished at a given instant.
    fn record(id: JobId, finished_at: Option<UnixNanos>) -> Arc<JobRecord> {
        Arc::new(JobRecord {
            snapshot: Mutex::new(JobSnapshot {
                id,
                key: key(),
                range: TimeRange::new(UnixNanos::EPOCH, UnixNanos::EPOCH).expect("empty is valid"),
                phase: if finished_at.is_some() {
                    Phase::Done
                } else {
                    Phase::Downloading
                },
                chunks_total: 1,
                chunks_done: 0,
                bars_written: 0,
                started_at: None,
                estimate: None,
                last_error: None,
                priority: Priority::Visible,
            }),
            cancelled: AtomicBool::new(false),
            finished_at: Mutex::new(finished_at),
        })
    }

    fn nanos(n: i64) -> UnixNanos {
        UnixNanos::from_nanos(n)
    }

    #[test]
    fn a_job_that_finished_long_ago_is_dropped() {
        let now = nanos(FINISHED_JOB_RETENTION_NANOS * 10);
        let mut jobs: HashMap<JobId, Arc<JobRecord>> = (0..500)
            .map(|i| {
                let id = JobId(i);
                (id, record(id, Some(nanos(0))))
            })
            .collect();

        retire_finished(&mut jobs, now);

        assert!(jobs.is_empty(), "{} finished jobs were kept", jobs.len());
    }

    #[test]
    fn a_job_that_just_finished_can_still_be_polled() {
        // `GET /api/bars/jobs/{id}` is a poll. A caller that started a job
        // has to see it reach `Done` at least once — dropping the record the
        // instant the work ended answers "no such job", which is what a made
        // up id also answers.
        let finished = nanos(FINISHED_JOB_RETENTION_NANOS * 10);
        let now = nanos(FINISHED_JOB_RETENTION_NANOS * 10 + 1);
        let id = JobId(1);
        let mut jobs = HashMap::from([(id, record(id, Some(finished)))]);

        retire_finished(&mut jobs, now);

        assert!(jobs.contains_key(&id));
    }

    #[test]
    fn a_running_job_is_never_retired_however_long_it_takes() {
        // A multi-day backfill is not stale, it is busy. Retiring by age
        // alone would drop the one job a caller is most likely waiting on.
        let now = nanos(FINISHED_JOB_RETENTION_NANOS * 1_000);
        let id = JobId(7);
        let mut jobs = HashMap::from([(id, record(id, None))]);

        retire_finished(&mut jobs, now);

        assert!(jobs.contains_key(&id), "a still-running job was retired");
    }

    #[test]
    fn retiring_keeps_the_running_jobs_and_drops_only_the_stale_ones() {
        let now = nanos(FINISHED_JOB_RETENTION_NANOS * 10);
        let running = JobId(1);
        let fresh = JobId(2);
        let stale = JobId(3);
        let mut jobs = HashMap::from([
            (running, record(running, None)),
            (fresh, record(fresh, Some(now))),
            (stale, record(stale, Some(nanos(0)))),
        ]);

        retire_finished(&mut jobs, now);

        let mut kept: Vec<u64> = jobs.keys().map(|id| id.0).collect();
        kept.sort_unstable();
        assert_eq!(kept, vec![1, 2]);
    }
}

#[cfg(test)]
mod tests {
    use super::SeriesLoaderBuilder;
    use crate::SystemClock;
    use crate::clock::test_support::ManualClock;
    use crate::job::{JobOutcome, Priority};
    use crate::source::{BarSource, FetchError};
    use async_trait::async_trait;
    use senken_core::{TimeRange, UnixNanos};
    use senken_series::{Anchor, Bar, BarSpec, BarUnit, Origin, SeriesKey};
    use senken_store::Store;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::time::Duration;
    use tempfile::TempDir;

    fn secs_range(start: i64, end: i64) -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(start).unwrap(),
            UnixNanos::from_secs(end).unwrap(),
        )
        .unwrap()
    }

    /// One bar per `spec`-aligned bucket start covering `range`, ascending.
    fn m1_bars_for(range: TimeRange, spec: BarSpec) -> Vec<Bar> {
        let step = spec
            .duration_nanos()
            .expect("test specs always have a fixed duration");
        let mut bars = Vec::new();
        let mut t = range.start().as_nanos();
        while t < range.end().as_nanos() {
            bars.push(Bar {
                ts_open: UnixNanos::from_nanos(t),
                open: 1,
                high: 1,
                low: 1,
                close: 1,
                volume: senken_series::Volume::Real(1),
                quote_volume: None,
                trade_count: None,
                taker_buy_volume: None,
            });
            t += step;
        }
        bars
    }

    /// Counts every `bars()` call and sleeps briefly first, so two
    /// concurrent callers racing for the same chunk both have time to
    /// reach the single-flight guard before either's fetch resolves.
    struct CountingSource {
        calls: AtomicU32,
        max_rows: usize,
        delay: Duration,
    }

    #[async_trait]
    impl BarSource for CountingSource {
        fn source_id(&self) -> &'static str {
            "binance-spot"
        }

        fn max_rows(&self) -> usize {
            self.max_rows
        }

        async fn bars(
            &self,
            _symbol: &str,
            spec: BarSpec,
            range: TimeRange,
        ) -> Result<Vec<Bar>, FetchError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            Ok(m1_bars_for(range, spec))
        }
    }

    /// Fails its first `fail_first` calls with a retryable error, then
    /// succeeds — for exercising retry/backoff without a real venue.
    struct FlakySource {
        attempts: AtomicU32,
        fail_first: u32,
    }

    /// Starts with a short historical response, then can return a full page
    /// that reaches farther back. `empty` makes the venue answer a whole page
    /// with nothing, which is the only response this loader accepts as proof
    /// that no older history exists; `full_response` distinguishes a complete
    /// page from a short one.
    struct BoundarySource {
        full_response: AtomicBool,
        empty: AtomicBool,
    }

    #[async_trait]
    impl BarSource for BoundarySource {
        fn source_id(&self) -> &'static str {
            "okx"
        }

        fn max_rows(&self) -> usize {
            2
        }

        async fn bars(
            &self,
            _symbol: &str,
            spec: BarSpec,
            range: TimeRange,
        ) -> Result<Vec<Bar>, FetchError> {
            if self.empty.load(Ordering::SeqCst) {
                return Ok(Vec::new());
            }
            let mut bars = m1_bars_for(range, spec);
            if !self.full_response.load(Ordering::SeqCst) {
                bars.truncate(1);
            }
            Ok(bars)
        }
    }

    #[async_trait]
    impl BarSource for FlakySource {
        fn source_id(&self) -> &'static str {
            "binance-spot"
        }

        fn max_rows(&self) -> usize {
            10_000
        }

        async fn bars(
            &self,
            _symbol: &str,
            spec: BarSpec,
            range: TimeRange,
        ) -> Result<Vec<Bar>, FetchError> {
            let attempt = self.attempts.fetch_add(1, Ordering::SeqCst);
            if attempt < self.fail_first {
                return Err(FetchError::Transient("simulated 429".to_owned()));
            }
            Ok(m1_bars_for(range, spec))
        }
    }

    /// Panics if ever called — for proving `plan()` never fetches.
    struct PanicSource;

    #[async_trait]
    impl BarSource for PanicSource {
        fn source_id(&self) -> &'static str {
            "binance-spot"
        }

        fn max_rows(&self) -> usize {
            1_000
        }

        async fn bars(
            &self,
            _symbol: &str,
            _spec: BarSpec,
            _range: TimeRange,
        ) -> Result<Vec<Bar>, FetchError> {
            panic!("plan() must never fetch");
        }
    }

    fn m1() -> BarSpec {
        BarSpec::new(1, BarUnit::Minute)
    }

    fn boundary_loader(source: &Arc<BoundarySource>) -> (TempDir, crate::SeriesLoader) {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let loader = SeriesLoaderBuilder::new(
            store,
            Arc::clone(source) as Arc<dyn BarSource>,
            Arc::new(SystemClock),
            m1(),
        )
        .finer_specs(vec![BarSpec::new(1, BarUnit::Hour)])
        .build();
        (dir, loader)
    }

    #[tokio::test]
    async fn a_whole_page_that_comes_back_empty_records_the_edge_per_spec_and_a_later_bar_revokes_it()
     {
        let source = Arc::new(BoundarySource {
            full_response: AtomicBool::new(true),
            empty: AtomicBool::new(true),
        });
        let (_dir, loader) = boundary_loader(&source);
        let m1_key = SeriesKey::new("okx", "BTCUSDT", Origin::Venue, m1());
        let h1_key = SeriesKey::new(
            "okx",
            "BTCUSDT",
            Origin::Venue,
            BarSpec::new(1, BarUnit::Hour),
        );

        // `max_rows` is 2, so 120 seconds of M1 is exactly one whole page.
        let edge_range = secs_range(600, 720);
        let outcome = loader
            .ensure(&m1_key, edge_range, Anchor::UTC, 0, 0, Priority::Prefetch)
            .wait()
            .await;
        assert!(matches!(outcome, JobOutcome::Completed));
        assert_eq!(
            loader.earliest_available(&m1_key, Anchor::UTC).unwrap(),
            Some(UnixNanos::from_secs(720).unwrap()),
            "a whole page answered with nothing is the venue saying it has no more"
        );
        assert_eq!(
            loader.earliest_available(&h1_key, Anchor::UTC).unwrap(),
            None,
            "a boundary observed for M1 must not be inherited by H1"
        );

        source.empty.store(false, Ordering::SeqCst);
        let earlier = secs_range(0, 120);
        let outcome = loader
            .ensure(&m1_key, earlier, Anchor::UTC, 0, 0, Priority::Prefetch)
            .wait()
            .await;
        assert!(matches!(outcome, JobOutcome::Completed));
        assert_eq!(
            loader.earliest_available(&m1_key, Anchor::UTC).unwrap(),
            None,
            "a real bar older than the recorded edge disproves it"
        );
    }

    /// The trailing chunk of any span that is not a whole multiple of a page
    /// is narrower than a page by construction (`split_into_chunks`), so it
    /// can never return a page's worth of rows. Counting its rows as evidence
    /// of the venue's left edge is what made scrolling back stop dead while
    /// the venue still had years of history.
    #[tokio::test]
    async fn a_chunk_narrower_than_a_page_never_claims_the_edge() {
        let source = Arc::new(BoundarySource {
            full_response: AtomicBool::new(true),
            empty: AtomicBool::new(true),
        });
        let (_dir, loader) = boundary_loader(&source);
        let m1_key = SeriesKey::new("okx", "BTCUSDT", Origin::Venue, m1());

        // 60 seconds is one M1 bar; `max_rows` is 2, so this is half a page.
        let outcome = loader
            .ensure(
                &m1_key,
                secs_range(600, 660),
                Anchor::UTC,
                0,
                0,
                Priority::Prefetch,
            )
            .wait()
            .await;
        assert!(matches!(outcome, JobOutcome::Completed));
        assert_eq!(
            loader.earliest_available(&m1_key, Anchor::UTC).unwrap(),
            None,
            "a chunk that could not hold a whole page proves nothing about the venue"
        );
    }

    /// One minute with no trades produces exactly the same short response as
    /// the true start of history. Treating it as the edge is unrecoverable:
    /// the client stops asking, so nothing ever arrives to revoke it.
    #[tokio::test]
    async fn a_whole_page_that_returns_some_bars_never_claims_the_edge() {
        let source = Arc::new(BoundarySource {
            full_response: AtomicBool::new(false),
            empty: AtomicBool::new(false),
        });
        let (_dir, loader) = boundary_loader(&source);
        let m1_key = SeriesKey::new("okx", "BTCUSDT", Origin::Venue, m1());

        let outcome = loader
            .ensure(
                &m1_key,
                secs_range(600, 720),
                Anchor::UTC,
                0,
                0,
                Priority::Prefetch,
            )
            .wait()
            .await;
        assert!(matches!(outcome, JobOutcome::Completed));
        assert_eq!(
            loader.earliest_available(&m1_key, Anchor::UTC).unwrap(),
            None,
            "a short but non-empty page is a gap or the edge, and the two are indistinguishable"
        );
    }

    /// Required test: "two concurrent requests at different
    /// timeframes for the same symbol issue exactly one fetch."
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn two_concurrent_requests_at_different_timeframes_issue_exactly_one_fetch() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let m15 = BarSpec::new(15, BarUnit::Minute);
        let h1 = BarSpec::new(1, BarUnit::Hour);
        let source = Arc::new(CountingSource {
            calls: AtomicU32::new(0),
            max_rows: 10_000,
            delay: Duration::from_millis(30),
        });
        let loader =
            SeriesLoaderBuilder::new(store, Arc::clone(&source) as _, Arc::new(SystemClock), m1())
                .finer_specs(vec![m1()])
                .build();

        let range = secs_range(0, 3600);
        let h1_key = SeriesKey::new("binance-spot", "BTCUSDT", Origin::Derived, h1);
        let m15_key = SeriesKey::new("binance-spot", "BTCUSDT", Origin::Derived, m15);

        // Both `ensure` calls are non-blocking (they only spawn), so both
        // jobs are already racing for the same M1 gap before either is
        // awaited below.
        let h1_handle = loader.ensure(&h1_key, range, Anchor::UTC, 0, 0, Priority::Visible);
        let m15_handle = loader.ensure(&m15_key, range, Anchor::UTC, 0, 0, Priority::Visible);

        let (h1_outcome, m15_outcome) = tokio::join!(h1_handle.wait(), m15_handle.wait());
        assert!(matches!(h1_outcome, JobOutcome::Completed));
        assert!(matches!(m15_outcome, JobOutcome::Completed));

        assert_eq!(
            source.calls.load(Ordering::SeqCst),
            1,
            "both jobs need the same missing M1 chunk; single-flight must collapse them into one fetch"
        );
        assert_eq!(loader.single_flight_fetch_starts(), 1);
    }

    /// Required test: "cancellation leaves already-written
    /// chunks intact."
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancellation_leaves_already_written_chunks_intact() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        // `max_rows` of 10 splits a 60-minute gap into 6 ten-minute chunks.
        let source = Arc::new(CountingSource {
            calls: AtomicU32::new(0),
            max_rows: 10,
            delay: Duration::from_millis(50),
        });
        let loader =
            SeriesLoaderBuilder::new(store.clone(), source, Arc::new(SystemClock), m1()).build();

        let key = SeriesKey::new("binance-spot", "BTCUSDT", Origin::Venue, m1());
        let range = secs_range(0, 3600);
        let handle = loader.ensure(&key, range, Anchor::UTC, 0, 0, Priority::Visible);
        let id = handle.id();

        loop {
            if let Some(snapshot) = loader.job(id)
                && snapshot.chunks_done >= 1
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        loader.cancel(id);
        let outcome = handle.wait().await;
        assert!(matches!(outcome, JobOutcome::Cancelled));

        let coverage = store.coverage(&key, Anchor::UTC).unwrap();
        let covered_secs: i64 = coverage
            .iter()
            .map(|r| r.end().as_nanos() / 1_000_000_000 - r.start().as_nanos() / 1_000_000_000)
            .sum();
        assert!(
            covered_secs > 0,
            "at least one chunk must have been written before cancellation"
        );
        assert!(
            covered_secs < 3600,
            "cancellation must have stopped the job before every chunk was written"
        );
    }

    /// Required test: "`plan()` performs no I/O beyond reading
    /// coverage, and starts no work — assert it, do not merely believe
    /// it." A file whose *name* declares full coverage but whose
    /// *contents* are not valid Parquet proves `plan()` never opened it —
    /// the same technique `senken-store`'s own tests use for
    /// `Store::coverage`.
    #[tokio::test]
    async fn plan_performs_no_io_beyond_coverage_and_starts_no_work() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let key = SeriesKey::new("binance-spot", "BTCUSDT", Origin::Venue, m1());
        let range = secs_range(0, 60);

        let bars_dir = dir
            .path()
            .join("sources/binance-spot/instruments/BTCUSDT/bars/venue-1m");
        std::fs::create_dir_all(&bars_dir).unwrap();
        std::fs::write(
            bars_dir.join(format!("{}.parquet", senken_store::encode_range(range))),
            b"not parquet",
        )
        .unwrap();

        let loader =
            SeriesLoaderBuilder::new(store, Arc::new(PanicSource), Arc::new(SystemClock), m1())
                .build();
        let requirement = loader.plan(&key, range, Anchor::UTC).unwrap();
        assert_eq!(requirement.missing, Vec::new());
        assert_eq!(requirement.covered, vec![range]);
        assert_eq!(requirement.chunks, 0);
        assert_eq!(loader.jobs().len(), 0, "plan() must not start any job");
    }

    #[test]
    fn an_uncovered_h1_request_fetches_native_h1_in_three_chunks() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let h1 = BarSpec::new(1, BarUnit::Hour);
        let loader = SeriesLoaderBuilder::new(
            store,
            Arc::new(CountingSource {
                calls: AtomicU32::new(0),
                max_rows: 100,
                delay: Duration::ZERO,
            }),
            Arc::new(SystemClock),
            m1(),
        )
        .finer_specs(vec![h1])
        .build();
        let key = SeriesKey::new("okx-spot", "BTCUSDT", Origin::Derived, h1);
        let range = secs_range(0, 300 * 3600);

        let requirement = loader.plan(&key, range, Anchor::UTC).unwrap();

        assert_eq!(requirement.chunks, 3);
        assert_eq!(requirement.estimated_bars, 300);
    }

    /// Required test: "a backfill invalidates a stale derived
    /// cache entry through the generation counter" — proven through the
    /// full `SeriesLoader` (`ensure` + `resolve`), not just the internal
    /// tracker.
    #[tokio::test]
    async fn a_write_to_the_dependency_invalidates_a_cached_derived_entry() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let h1 = BarSpec::new(1, BarUnit::Hour);
        let source = Arc::new(CountingSource {
            calls: AtomicU32::new(0),
            max_rows: 10_000,
            delay: Duration::ZERO,
        });
        let loader = SeriesLoaderBuilder::new(store, source, Arc::new(SystemClock), m1())
            .finer_specs(vec![m1()])
            .build();

        let hour1 = secs_range(0, 3600);
        let hour2 = secs_range(3600, 7200);
        let derived_key = SeriesKey::new("binance-spot", "BTCUSDT", Origin::Derived, h1);

        loader
            .ensure(&derived_key, hour1, Anchor::UTC, 0, 0, Priority::Visible)
            .wait()
            .await;
        loader
            .resolve(&derived_key, hour1, Anchor::UTC)
            .await
            .unwrap();

        let misses_before_repeat = loader.cache_metrics().misses;
        let hits_before_repeat = loader.cache_metrics().hits;
        loader
            .resolve(&derived_key, hour1, Anchor::UTC)
            .await
            .unwrap();
        assert_eq!(
            loader.cache_metrics().misses,
            misses_before_repeat,
            "an identical repeat request must not miss"
        );
        assert!(
            loader.cache_metrics().hits > hits_before_repeat,
            "an identical repeat request must hit the cache"
        );

        // The "backfill": a disjoint fetch against the *same* underlying
        // M1 series. Design its generation counter is per series, not
        // per overlapping range, so this must invalidate the hour-1
        // derived entry too, even though hour 2 does not overlap it.
        loader
            .ensure(&derived_key, hour2, Anchor::UTC, 0, 0, Priority::Visible)
            .wait()
            .await;

        let misses_before_final = loader.cache_metrics().misses;
        loader
            .resolve(&derived_key, hour1, Anchor::UTC)
            .await
            .unwrap();
        assert!(
            loader.cache_metrics().misses > misses_before_final,
            "a write to the dependency must invalidate the earlier cached derived entry, not silently serve it stale"
        );
    }

    /// A `429`-shaped transient failure is retried and the job still
    /// completes — "`last_error` while retrying is not `Failed`"
    ///   — proven with a fully controlled [`ManualClock`] so the
    /// retry backoff never performs a real wait.
    #[tokio::test]
    async fn a_transient_failure_is_retried_without_failing_the_job() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let source = Arc::new(FlakySource {
            attempts: AtomicU32::new(0),
            fail_first: 2,
        });
        let clock = Arc::new(ManualClock::at(0));
        let loader = SeriesLoaderBuilder::new(store, Arc::clone(&source) as _, clock, m1()).build();

        let key = SeriesKey::new("binance-spot", "BTCUSDT", Origin::Venue, m1());
        let range = secs_range(0, 60);
        let outcome = loader
            .ensure(&key, range, Anchor::UTC, 0, 0, Priority::Visible)
            .wait()
            .await;

        assert!(
            matches!(outcome, JobOutcome::Completed),
            "retries must let the job complete, not fail it while the source is only transiently failing"
        );
        assert_eq!(source.attempts.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn builder_uses_the_documented_default_cache_budget() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        let loader =
            SeriesLoaderBuilder::new(store, Arc::new(PanicSource), Arc::new(SystemClock), m1())
                .build();
        assert_eq!(loader.cache_metrics().max_bytes, super::DEFAULT_CACHE_BYTES);
    }

    /// Records which symbol's `bars()` call *started*, in order — the
    /// instant a call starts is exactly the instant the loader's
    /// concurrency gate granted it a slot, so this order is a direct
    /// observation of what the gate decided. Only the very
    /// first call blocks (on `release_first`), so the test controls
    /// exactly when the sole slot it holds becomes contested.
    struct GatedSource {
        max_rows: usize,
        order: std::sync::Mutex<Vec<String>>,
        call_count: AtomicU32,
        release_first: tokio::sync::Notify,
    }

    #[async_trait]
    impl BarSource for GatedSource {
        fn source_id(&self) -> &'static str {
            "binance-spot"
        }

        fn max_rows(&self) -> usize {
            self.max_rows
        }

        async fn bars(
            &self,
            symbol: &str,
            spec: BarSpec,
            range: TimeRange,
        ) -> Result<Vec<Bar>, FetchError> {
            self.order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(symbol.to_owned());
            if self.call_count.fetch_add(1, Ordering::SeqCst) == 0 {
                self.release_first.notified().await;
            }
            Ok(m1_bars_for(range, spec))
        }
    }

    /// Polls `cond` until true, yielding between checks — deterministic
    /// synchronisation on a known state rather than a guessed sleep
    /// duration, wrapped in a generous real-time safety net purely so a
    /// genuine bug fails with a clear timeout instead of hanging the suite.
    async fn wait_until(mut cond: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if cond() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("condition was not met before the test's safety timeout");
    }

    /// Required test: "prove [priority scheduling]
    /// with a test before trusting it." A single concurrent slot forces a
    /// real contest: Background job **A**'s one chunk takes the empty gate
    /// and is held open; Background job **B**'s one chunk then queues
    /// behind it (confirmed via `fetch_gate_waiting_count() == 1`, polled
    /// deterministically, never a sleep) — so far this is indistinguishable
    /// from plain FIFO. Only *then* does Visible job **V**'s one chunk also
    /// queue (confirmed `== 2`), arriving strictly *after* B. Releasing A's
    /// chunk now presents the gate with two simultaneous waiters, B
    /// (queued first) and V (queued second, higher priority) — the one
    /// scenario that actually distinguishes the two policies: a FIFO gate
    /// must grant B next (it queued first); a priority gate must grant V
    /// next regardless of arrival order. The pre-M8.3 `Semaphore` would
    /// produce `[A, B, V]` here; this loader must produce `[A, V, B]`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_visible_jobs_chunk_is_serviced_before_an_earlier_queued_backgrounds_chunk() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let source = Arc::new(GatedSource {
            max_rows: 10_000,
            order: std::sync::Mutex::new(Vec::new()),
            call_count: AtomicU32::new(0),
            release_first: tokio::sync::Notify::new(),
        });
        let loader = SeriesLoaderBuilder::new(
            store,
            Arc::clone(&source) as Arc<dyn BarSource>,
            Arc::new(SystemClock),
            m1(),
        )
        .max_concurrent_fetches(1)
        .build();

        let one_chunk = secs_range(0, 60);
        let key_for = |symbol: &str| SeriesKey::new("binance-spot", symbol, Origin::Venue, m1());

        let a_handle = loader.ensure(
            &key_for("AAA"),
            one_chunk,
            Anchor::UTC,
            0,
            0,
            Priority::Background,
        );
        // A's chunk has the sole slot and is blocked in `bars()`.
        let order_len = || {
            source
                .order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len()
        };
        wait_until(|| order_len() >= 1).await;

        let b_handle = loader.ensure(
            &key_for("BBB"),
            one_chunk,
            Anchor::UTC,
            0,
            0,
            Priority::Background,
        );
        // B is confirmed queued *before* V even exists.
        wait_until(|| loader.fetch_gate_waiting_count() == 1).await;

        let v_handle = loader.ensure(
            &key_for("VVV"),
            one_chunk,
            Anchor::UTC,
            0,
            0,
            Priority::Visible,
        );
        // Both B and V are confirmed queued together, B first — the exact
        // contested state this test exists to release into.
        wait_until(|| loader.fetch_gate_waiting_count() == 2).await;

        source.release_first.notify_one();

        let a_outcome = a_handle.wait().await;
        let b_outcome = b_handle.wait().await;
        let v_outcome = v_handle.wait().await;
        assert!(matches!(a_outcome, JobOutcome::Completed));
        assert!(matches!(b_outcome, JobOutcome::Completed));
        assert!(matches!(v_outcome, JobOutcome::Completed));

        let order = source
            .order
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        assert_eq!(
            order,
            vec!["AAA", "VVV", "BBB"],
            "V must be granted the slot ahead of B despite queueing after it — B queuing first is exactly what a FIFO gate would have honoured instead"
        );
    }
}
