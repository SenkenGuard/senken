//! [`LoadError`] — everything that can go wrong resolving, planning or
//! fetching bars through a [`crate::SeriesLoader`].

use senken_series::AggregateError;
use senken_store::StoreError;

use crate::source::FetchError;

/// Why a [`crate::SeriesLoader`] call failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LoadError {
    /// Reading or writing the underlying [`senken_store::Store`] failed.
    #[error(transparent)]
    Store(#[from] StoreError),

    /// A [`crate::BarSource::bars`] call failed and exhausted its retries.
    #[error(transparent)]
    Fetch(#[from] FetchError),

    /// The requested spec pair cannot be aggregated —
    /// [`senken_series::divides`] disagreed. Surfaces when a candidate
    /// spec this loader was configured with turns out not to actually
    /// divide the requested target, which a correctly configured loader
    /// should never hit; kept as a reported error rather than a panic
    /// because a caller can still misconfigure the candidate list.
    #[error(transparent)]
    Aggregate(#[from] AggregateError),

    /// A job's own task ended without reporting an outcome (a panic inside
    /// it). Reported through [`crate::JobHandle::wait`] as
    /// [`crate::JobOutcome::Failed`] rather than surfacing as a distinct
    /// failure shape a caller would have to special-case.
    #[error("the job's task ended without reporting an outcome")]
    JobPanicked,
}
