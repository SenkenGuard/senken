//! Bar and trade types, timeframe specs, and M1-to-N aggregation.
//!
//! **Pure computation. No I/O, no Arrow, no Parquet, no network, and no
//! wall-clock reads anywhere** — this crate depends only on `senken-core`,
//! `serde`, `thiserror`, and `async-trait` (for [`Clock`]'s sake alone).
//! Storage (`senken-store`) and scheduling (`senken-loader`, plan
//! build on top of this; neither Arrow nor a filesystem nor a runtime
//! ever needs to appear here for a consumer who only wants bar types and
//! aggregation.
//!
//! # What lives here
//!
//! - [`BarSpec`]/[`BarUnit`] — an open timeframe spec, not a closed enum,
//!   so finer units (volume, tick, Renko bars) can be added later with no
//!   schema change.
//! - [`Origin`] — whether a bar came from the venue or was aggregated here;
//!   part of [`SeriesKey`]'s identity, not a side annotation, because a
//!   venue bar and a locally-derived bar for the same symbol are different
//!   data with different biases.
//! - [`Bar`]/[`Trade`] — plain scaled `i64` fields; the scale itself is a
//!   property of the *series*, written to file metadata in `senken-store`,
//!   never a field on `Bar` itself.
//! - [`divides`]/[`bucket_start`]/[`Aggregator`] — the aggregation rules,
//!   including the non-negotiable one: a partially-aggregated bar is never
//!   emitted (see the `aggregate` module docs for exactly how that is
//!   enforced).
//! - [`Clock`] — introduced here, not with a future trade engine, because
//!   backtest, replay and live trading differ only in where time comes
//!   from, and a wall-clock read anywhere in this stack would make every
//!   later consumer of it non-deterministic.

mod aggregate;
mod bar;
mod clock;
mod spec;

pub use crate::aggregate::{
    AggregateError, Aggregator, Anchor, bucket_start, divides, next_bucket_start,
};
pub use crate::bar::{Bar, BarPriceBasis, SeriesKey, Side, Trade, Volume};
pub use crate::clock::Clock;
pub use crate::spec::{BarSpec, BarUnit, Origin, ParseBarSpecError, ParseOriginError};
