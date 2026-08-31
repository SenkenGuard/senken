//! The resolution ladder between a chart and [`senken_store::Store`].
//!
//! # What lives here: the resolution ladder, cache, jobs and single-flight
//!
//! - [`SeriesLoader`] — memory cache → store at the exact spec → aggregate
//!   from a stored finer spec (coarsest first) → fetch the gaps.
//! - Single-flight fetching keyed on the **chunk**, `(source, symbol,
//!   base_spec, chunk_range)`, never on the caller's request
//!   — two charts deriving different target specs from the same
//!   missing base data collapse into one fetch.
//! - Byte-bounded LRU caching with a generation counter per series
//!   — a backfill to one series invalidates every derived
//!   cache entry that depended on it, without an invalidation graph.
//! - Progressive, cancellable, prioritised job scheduling with Parquet
//!   decode and aggregation kept off async worker threads
//!   — priority is not merely reported but actually scheduled
//!   cross-job: a `Visible` job's chunk is serviced
//!   ahead of a running `Background` job's, via `priority_gate`'s
//!   `PriorityGate`.
//! - Stateful, observable jobs: `plan()`/`ensure()` are separate calls —
//!   inspecting what is missing never starts work.
//! - Stitched resolution: a `Derived` request no
//!   longer needs one candidate spec to cover its entire range — adjacent
//!   regions covered by different stored specs are combined, never at the
//!   cost of the "no partial bucket" rule (`ladder::trim_to_whole_buckets`).
//!
//! # Two `BarSource` traits
//!
//! [`BarSource`] here is this crate's own small fetch port (see its module
//! docs), predating `senken-plugin`'s real, plugin-facing trait of the
//! same name — every test in this crate exercises it against an in-memory
//! fake, never a network call. [`PluginBarSource`] is the one documented
//! adapter (see its module docs) that lets a real
//! `senken_plugin::BarSource` implementation satisfy this port, so the two
//! traits stay reconciled rather than silently diverging.
//!
//! # No wall-clock reads
//!
//! Every piece of this crate that needs "now" — job timestamps, throughput
//! measurement, retry backoff — takes a [`senken_series::Clock`] and reads
//! it, rather than `SystemTime::now()`/`Instant::now()` directly; the one
//! exception is [`SystemClock`], the concrete real-time implementation
//! this crate provides (see its own module docs for why here, not
//! `senken-series`).

mod cache;
mod chunk;
mod clock;
mod coverage;
mod error;
mod generation;
mod job;
mod ladder;
mod loader;
mod plugin_adapter;
mod priority_gate;
mod source;

pub use crate::cache::CacheMetrics;
pub use crate::clock::SystemClock;
pub use crate::error::LoadError;
pub use crate::job::{
    JobHandle, JobId, JobOutcome, JobSnapshot, ParseJobIdError, Phase, Priority, Requirement,
};
pub use crate::loader::{
    DEFAULT_CACHE_BYTES, DEFAULT_MAX_CONCURRENT_FETCHES, Resolved, SeriesLoader,
    SeriesLoaderBuilder,
};
pub use crate::plugin_adapter::PluginBarSource;
pub use crate::source::{BarSource, FetchError};
