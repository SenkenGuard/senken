//! Everything that can go wrong building, evaluating or persisting an
//! alert.

/// Why building a concrete indicator from a stored `(name, params)` pair
/// failed, or why an [`crate::IndicatorField`](crate::condition::IndicatorField)
/// does not apply to the indicator it was asked to read from.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IndicatorSpecError {
    /// `name` did not match any of the ten built-ins.
    #[error("unknown indicator {0:?}")]
    UnknownIndicator(String),
    /// `params` was not valid JSON, or was valid JSON missing a field the
    /// named indicator requires.
    #[error("invalid parameters for {indicator}: {reason}")]
    InvalidParams {
        /// The indicator whose parameters could not be read.
        indicator: String,
        /// Why the parameters were rejected.
        reason: String,
    },
    /// The requested [`crate::condition::IndicatorField`] is not one this
    /// indicator reports (e.g. asking an `Sma` for
    /// [`crate::condition::IndicatorField::MacdLine`]).
    #[error("{indicator} does not report field {field:?}")]
    FieldNotReported {
        /// The indicator that was asked.
        indicator: &'static str,
        /// The field it does not report.
        field: crate::condition::IndicatorField,
    },
}

/// Why an alert-store operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AlertError {
    /// The underlying SQLite call failed.
    #[error("sqlite operation failed")]
    Database(#[from] rusqlite::Error),
    /// The permission check itself — reused from `senken-identity` so a
    /// caller sees the exact same error that crate's and
    /// `senken-workspace`'s own guarded queries produce for the same
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
