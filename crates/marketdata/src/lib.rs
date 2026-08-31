//! Market data domain types, the source contract, and a cached multi-source
//! instrument catalog.
//!
//! [`MarketData`] is the entry point: register any number of
//! [`MarketDataSource`]s, then search across all of them with
//! [`instruments`](MarketData::instruments) or address one instrument by its
//! [`InstrumentId`]. Each source's catalog is fetched at most once, cached on
//! disk through [`senken_storage`], and served from memory after that.
//!
//! The async methods must be called from within a Tokio runtime — any
//! flavour, including `current_thread` — because cached catalogs are read
//! and written on Tokio's blocking thread pool.
//!
//! See the crate README for a walkthrough and [`Instrument`] for the
//! fixed-point contract every source honours.
//!
//! # Cargo features
//!
//! * `registry` *(default)* — everything described above. Without default
//!   features the crate is only the domain vocabulary — [`Instrument`],
//!   [`InstrumentId`], [`InstrumentQuery`], the [`decimal`] helpers, and
//!   [`paths`] (the on-disk layout) — with serde as its
//!   heaviest dependency.

pub mod id;
pub mod instrument;
pub mod paths;
pub mod query;

#[cfg(feature = "registry")]
pub mod catalog;
#[cfg(feature = "registry")]
mod registry;
#[cfg(feature = "registry")]
pub mod source;

// The decimal-string helpers moved to `senken-core`
// . Re-exported under the same module path so no plugin source needed
// to change: `senken_marketdata::decimal::parse_increment` etc. still
// resolve, now to `senken_core`'s implementation.
pub use senken_core::decimal;

pub use crate::decimal::{decimal_places, format_scaled, parse_increment, parse_scaled};
pub use crate::id::{ID_SEPARATOR, InstrumentId, InstrumentIdError};
pub use crate::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, OptionRight, OptionTerms, Settlement,
    SourceSymbol,
};
pub use crate::paths::{instrument_dir, instruments_path, source_dir};
pub use crate::query::{InstrumentQuery, MatchRank};

#[cfg(feature = "registry")]
pub use crate::catalog::SourceCatalog;
#[cfg(feature = "registry")]
pub use crate::registry::{
    DEFAULT_CACHE_TTL, INSTRUMENTS_SCHEMA_VERSION, InstrumentMatch, InstrumentPage, MarketData,
    MarketDataError, SourceFailure,
};
#[cfg(feature = "registry")]
pub use crate::source::{MarketDataSource, SourceDetail, SourceError, SourceSummary};
