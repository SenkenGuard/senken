//! Job types: [`JobId`], [`Phase`], [`JobOutcome`],
//! [`Priority`], [`JobSnapshot`], [`Requirement`] and [`JobHandle`].
//!
//! Opening a chart with no data fans out into work lasting seconds to
//! minutes — "work out which base bars are missing, download them in
//! chunks under a rate limit, write Parquet, aggregate, render." That is a
//! job, and jobs are inspectable rather than hidden inside one opaque
//! future: progress is counted in chunks and bars (percent is a
//! presentation concern for whichever app renders this), an ETA is `None`
//! until real throughput has been measured rather than invented, and a
//! transient failure being retried is `last_error`, not `Failed` — showing
//! a backed-off `429` as a failure trains users to ignore errors.

use std::fmt;
use std::str::FromStr;
use std::time::Duration;

use senken_core::{TimeRange, UnixNanos};
use senken_series::SeriesKey;

use crate::error::LoadError;

/// Identifies one job. Opaque and monotonically assigned by a
/// [`crate::SeriesLoader`]; never reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct JobId(pub(crate) u64);

impl fmt::Display for JobId {
    /// The plain decimal counter, so a caller with no other way to name a
    /// job — an HTTP path segment, say (bars over HTTP) — has
    /// one. Job ids are unique per [`crate::SeriesLoader`], not globally, so
    /// a caller that talks to more than one loader must remember which one
    /// minted the id it holds.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// [`JobId::from_str`] rejects anything that is not a plain, non-negative
/// integer — the exact inverse of its [`fmt::Display`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0:?} is not a valid job id")]
pub struct ParseJobIdError(String);

impl FromStr for JobId {
    type Err = ParseJobIdError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse::<u64>()
            .map(JobId)
            .map_err(|_| ParseJobIdError(s.to_owned()))
    }
}

/// Where one job is in its lifecycle.
///
/// `Phase` tracks *progress*, not terminal success, failure or
/// cancellation: all three stop a job's phase progression at whichever
/// phase it was in (or advance it to [`Self::Done`] on a clean finish), and
/// [`JobOutcome`] — delivered once, through [`JobHandle::wait`] — is what
/// actually distinguishes them. The plan's own sketch defines
/// `JobSnapshot` with a `phase` field and defines `JobOutcome` as a
/// separate type, but does not show a `JobSnapshot` field carrying it; this
/// crate reads that as intentional (a snapshot is about *progress*,
/// `JobOutcome` is a one-time terminal event) rather than an omission to
/// silently fill in with a guess.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Enqueued, not yet started.
    Queued,
    /// Fetching a chunk from the [`crate::BarSource`].
    Downloading,
    /// Writing a fetched chunk to the store.
    Writing,
    /// Warming the derived cache for the originally requested spec, once
    /// every chunk needed to cover the request has been written.
    Aggregating,
    /// No longer running — see this type's own docs for how a cancelled or
    /// failed job also ends up here.
    Done,
}

/// How a finished job ended.
///
/// Not a [`JobSnapshot`] field (see [`Phase`]'s docs) — delivered once,
/// on completion, through [`JobHandle::wait`].
#[derive(Debug)]
pub enum JobOutcome {
    /// Every chunk needed to cover the requested range was fetched and
    /// written.
    Completed,
    /// A chunk fetch failed and exhausted its retries.
    Failed(LoadError),
    /// [`crate::SeriesLoader::cancel`] was called before every chunk
    /// finished. Chunks already written before cancellation took effect
    /// are kept ("anything already written is kept, since it is
    /// still true").
    Cancelled,
}

/// Where a job's chunks sit relative to other jobs': visible
/// range first, then adjacent prefetch, then background backfill.
///
/// Declaration order is significant for the derived [`Ord`]: later
/// variants rank higher, so `Priority::Visible > Priority::Prefetch >
/// Priority::Background`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Priority {
    /// Background backfill — serviced only once nothing higher-priority is
    /// pending.
    Background,
    /// Prefetching a range adjacent to what is currently on screen.
    Prefetch,
    /// The range currently on screen. Always serviced first.
    Visible,
}

/// A point-in-time view of one job.
#[derive(Debug, Clone)]
pub struct JobSnapshot {
    /// This job's id.
    pub id: JobId,
    /// The series this job is filling gaps for.
    pub key: SeriesKey,
    /// The range originally requested.
    pub range: TimeRange,
    /// Where this job currently is.
    pub phase: Phase,
    /// How many fetch chunks this job's plan requires in total.
    pub chunks_total: u32,
    /// How many of those chunks have been fetched and written so far.
    pub chunks_done: u32,
    /// How many bars have been written so far, across every chunk.
    pub bars_written: u64,
    /// When this job started running, per the [`senken_series::Clock`] it
    /// was given — `None` before it has actually started (still
    /// [`Phase::Queued`]).
    pub started_at: Option<UnixNanos>,
    /// An estimate of the remaining time, from this loader's measured
    /// throughput. `None` until at least one chunk has completed — an
    /// honest absence beats an invented number.
    pub estimate: Option<Duration>,
    /// Set while a chunk fetch is being retried after a transient failure.
    /// This is **not** the same as the job having failed — see this
    /// module's docs.
    pub last_error: Option<String>,
    /// This job's priority.
    pub priority: Priority,
}

/// What [`crate::SeriesLoader::plan`] found: pure inspection, no network
/// call, no work started, nothing mutated. This is what a
/// backtest preflight dialog renders as "3 months of M1 missing, ~1,300
/// requests, about 4 minutes — proceed?" *before* calling
/// [`crate::SeriesLoader::ensure`] to actually start the work.
#[derive(Debug, Clone)]
pub struct Requirement {
    /// The series inspected.
    pub key: SeriesKey,
    /// The range inspected.
    pub range: TimeRange,
    /// The parts of `range` already resolvable — from the memory cache,
    /// the store at the exact spec, or aggregation from an already-stored
    /// finer spec — without fetching anything.
    pub covered: Vec<TimeRange>,
    /// The parts of `range` that a call to
    /// [`crate::SeriesLoader::ensure`] would actually need to fetch.
    pub missing: Vec<TimeRange>,
    /// How many venue-page-sized fetch chunks `missing` splits into.
    pub chunks: u32,
    /// An estimate of how many bars `missing` represents, at the base spec
    /// a fetch would actually run at.
    pub estimated_bars: u64,
    /// An estimate of how long fetching `missing` would take, from this
    /// loader's already-measured throughput (shared with
    /// [`JobSnapshot::estimate`]). `None` if no throughput has been
    /// measured yet in this loader's lifetime.
    pub estimate: Option<Duration>,
}

/// A handle to one job just enqueued by [`crate::SeriesLoader::ensure`].
///
/// Dropping this handle does **not** cancel the job — jobs run
/// independently of who is watching them and stay visible through
/// [`crate::SeriesLoader::jobs`] regardless. Call
/// [`crate::SeriesLoader::cancel`] to actually stop one.
#[derive(Debug)]
pub struct JobHandle {
    pub(crate) id: JobId,
    pub(crate) outcome: tokio::sync::oneshot::Receiver<JobOutcome>,
}

impl JobHandle {
    /// This job's id, for [`crate::SeriesLoader::job`]/
    /// [`crate::SeriesLoader::cancel`].
    #[must_use]
    pub fn id(&self) -> JobId {
        self.id
    }

    /// Waits for the job to reach a terminal state and reports how it got
    /// there.
    ///
    /// Not part of the plan's own illustrative sketch: [`JobOutcome`] is
    /// defined there with no accessor named for it anywhere in `impl
    /// SeriesLoader`. A caller — this crate's own tests included — needs
    /// *some* way to observe it, so this is the minimal addition that
    /// provides one, added because the type it returns already exists in
    /// the plan.
    pub async fn wait(self) -> JobOutcome {
        self.outcome
            .await
            .unwrap_or(JobOutcome::Failed(LoadError::JobPanicked))
    }
}

#[cfg(test)]
mod tests {
    use super::JobId;

    #[test]
    fn a_job_id_round_trips_through_display_and_from_str() {
        let id = JobId(42);
        assert_eq!(id.to_string().parse::<JobId>().unwrap(), id);
    }

    #[test]
    fn from_str_rejects_anything_that_is_not_a_plain_integer() {
        assert!("not a number".parse::<JobId>().is_err());
        assert!("-1".parse::<JobId>().is_err());
        assert!("".parse::<JobId>().is_err());
    }
}
