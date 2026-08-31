//! Parquet-backed bar (and trade) storage: filename-derived
//! coverage, immutable non-overlapping files, and Arrow schema/write
//! assertions.
//!
//! # The Arrow boundary
//!
//! This is the **only** crate in the workspace allowed to depend on
//! `arrow`/`parquet`, and even here they are optional, behind the
//! `parquet` feature (enabled by default). Everything that does not
//! actually touch a Parquet file — filename encoding ([`range`],
//! `spec_token`), path construction ([`paths`]), and coverage inspection
//! ([`Store::coverage`]) — compiles and works with `default-features =
//! false`. A consumer who only wants to know what data exists on disk, or
//! who only wants `senken-series`' bar types, never has to pull in the
//! Arrow dependency graph. `cargo check -p senken-store
//! --no-default-features --all-targets` proves this boundary holds.
//!
//! # Coverage has no side table
//!
//! A file's name states the [`senken_core::TimeRange`] it was fetched for
//!: `coverage()` is a directory listing, nothing more, so
//! there is no second source of truth that can silently drift from the
//! first. The **filename** says what was fetched; the **rows** (once
//! [`Store::write`] and [`Store::read_range`] exist, under `parquet`) say
//! what existed — a gap inside a declared range is a real market gap, a
//! gap outside every range is simply unfetched. Neither is ever
//! synthesised.
//!
//! # Files are immutable
//!
//! Extending coverage always writes a *new* file and unlinks the
//! superseded one; nothing is ever rewritten in place. This
//! is what makes the Windows case safe — renaming over a file another
//! process holds open fails outright there — and it is why
//! [`Store::write`] reuses `senken-storage`'s atomic write discipline
//! (temp file → `sync_all` → rename → fsync directory) rather than
//! reinventing it.
//!
//! # The anchor is part of a persisted Day-or-above series' identity
//!
//! Measured live against OKX: `bar=1D` opens at
//! UTC+8 while `bar=1Dutc` opens at UTC — one endpoint, two different
//! series. A *venue-supplied* Day-or-above series therefore carries its
//! anchor in its storage path (`spec_token`), or two eight-hour-shifted
//! series would collide under one directory and interleave. Below `Day`
//! this never matters — an hour boundary needs no notion of "midnight" —
//! so [`senken_series::Anchor`] is silently ignored there.

pub mod paths;
pub mod range;
mod spec_token;
mod store;

mod error;

#[cfg(feature = "parquet")]
mod assertions;
#[cfg(feature = "parquet")]
mod reader;
#[cfg(feature = "parquet")]
mod schema;
#[cfg(feature = "parquet")]
mod writer;

pub use crate::error::{StoreError, WriteAssertionError};
pub use crate::range::{decode_range, encode_range};
pub use crate::spec_token::{decode_spec_token, encode_spec_token};
pub use crate::store::Store;

#[cfg(feature = "parquet")]
pub use crate::reader::bars_from_batch;
#[cfg(feature = "parquet")]
pub use crate::schema::{SCHEMA_VERSION, SeriesMetadata};
