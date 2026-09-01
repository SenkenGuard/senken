//! Everything that can go wrong building, evaluating or persisting an
//! alert.

/// Why building a concrete indicator from a stored `(name, params)` pair
/// failed, or why an [`crate::IndicatorField`](crate::condition::IndicatorField)
/// does not apply to the indicator it was asked to read from.
///
/// This crate keeps the name it has always used here, but the type itself
/// now lives in `senken-indicators` — the same dynamic build-and-read
/// contract a live indicator session (`senken-subscription`) needs, and
/// which must not depend on this crate (an alert already leases its series
/// through `senken-subscription`, so the reverse dependency would be a
/// cycle).
pub use senken_indicators::DynamicIndicatorError as IndicatorSpecError;

/// Why an alert-store operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AlertError {
    /// The underlying SQLite call failed.
    #[error("sqlite operation failed")]
    Database(#[from] rusqlite::Error),
    /// The permission check itself — reused from `senken-identity` so a
    /// caller sees the exact same error that crate's and
    /// `senken-chart`'s own guarded queries produce for the same
    /// reasons.
    #[error(transparent)]
    Identity(#[from] senken_identity::IdentityError),
    /// No alert exists for the given id.
    #[error("no alert found for that id")]
    AlertNotFound,
    /// A stored `instrument` column no longer parses as a
    /// `senken_marketdata::InstrumentId`.
    #[error("stored instrument id is corrupt: {0}")]
    CorruptInstrumentId(String),
    /// A stored `timeframe` column no longer parses as a
    /// `senken_series::BarSpec`.
    #[error("stored timeframe is corrupt: {0}")]
    CorruptTimeframe(String),
    /// A stored `condition_field`/`condition_comparator` column held a
    /// value this crate does not recognise.
    #[error("stored condition is corrupt: {0}")]
    CorruptCondition(String),
    /// Building the indicator this alert names failed — either at creation
    /// time (refusing to persist an alert that could never evaluate) or
    /// while loading a previously-stored row back.
    #[error(transparent)]
    IndicatorSpec(#[from] IndicatorSpecError),
}
