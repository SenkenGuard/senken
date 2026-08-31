//! [`BarSource`] — the fetch port this crate depends on.
//!
//! This layer needs no venue network at
//! all: there is no `BarSource` implementation yet, so define
//! the port you depend on and test it with an in-memory fake." This trait
//! is that port, not the final one — this crate owns "the
//! `Plugin`/`BarSource` contracts" to `senken-plugin`, and M7's
//! illustrative sketch adds a `supported()` method for
//! venue-registration purposes this crate has no use for. The shape below
//! is deliberately close to that sketch (`source_id`, `max_rows`, `bars`)
//! so a future M7 executor can widen this trait in place, or have
//! `senken-plugin`'s real one subsume it, without a redesign — but it is
//! this crate's own type until then.

use async_trait::async_trait;
use senken_core::TimeRange;
use senken_series::{Bar, BarSpec};

/// Fetches bars for one source. An implementation talks to exactly one
/// venue (or, in every test in this crate, an in-memory fake) and knows
/// nothing about caching, gap planning, single-flight or jobs — all of
/// that is `senken-loader`'s job.
#[async_trait]
pub trait BarSource: Send + Sync {
    /// The source id this fetches for, e.g. `binance-spot`. Must match the
    /// `source_id` of every [`senken_series::SeriesKey`] a caller passes
    /// alongside this source.
    fn source_id(&self) -> &str;

    /// The largest number of bars one [`Self::bars`] call may return. The
    /// loader never asks for a chunk wider than this many bars (plan
    /// its chunk sizing follows the venue's own page size — e.g.
    /// Binance spot's tested cap of 1000).
    fn max_rows(&self) -> usize;

    /// Fetches every **closed** bar of `spec` for `symbol` inside `range`,
    /// ascending by `ts_open`.
    ///
    /// A real implementation must already have dropped any unclosed
    /// candle and normalised to ascending order before
    /// returning — this crate trusts what it is given and does not
    /// re-check it, since M6 has no real implementation to enforce that
    /// against.
    ///
    /// # Errors
    /// [`FetchError`], whose [`FetchError::is_retryable`] tells the caller
    /// whether trying again is worth it.
    async fn bars(
        &self,
        symbol: &str,
        spec: BarSpec,
        range: TimeRange,
    ) -> Result<Vec<Bar>, FetchError>;
}

/// Why a [`BarSource::bars`] call failed.
///
/// Deliberately small and self-contained rather than reusing
/// `senken_marketdata::SourceError` or `senken_venue`'s retry machinery:
/// both belong to the instrument/HTTP layers a real M7 implementation sits
/// behind, and this port only needs to know whether retrying is worth it —
/// widening it to carry HTTP status codes or transport detail is an M7
/// concern once a real implementation exists to need them.
#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum FetchError {
    /// A transient failure (timeout, connection reset, 5xx, 429, ...) that
    /// is worth retrying.
    #[error("transient fetch failure: {0}")]
    Transient(String),
    /// A failure retrying cannot fix (bad symbol, unsupported spec, a 4xx
    /// other than 429, ...).
    #[error("rejected: {0}")]
    Rejected(String),
}

impl FetchError {
    /// Whether the caller should retry this fetch.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        matches!(self, Self::Transient(_))
    }
}
