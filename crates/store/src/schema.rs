//! The Arrow schema and file-level metadata for a bar Parquet file
//! .
//!
//! Every field name here (`senken.*`) is intentionally namespaced: the
//! file sits next to other Parquet files a user might point Polars or
//! `DuckDB` at directly, and an unprefixed `schema_version` key
//! would be an easy collision with someone else's metadata.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;

use arrow::datatypes::{DataType, Field, Schema, SchemaRef};
use senken_core::{TimeRange, UnixNanos};
use senken_series::{BarPriceBasis, BarSpec, Origin};

/// The current on-disk schema version. Bump this, and reset
/// coverage rather than migrate, on any incompatible column change — the
/// same discipline `senken-storage`'s `Snapshot` uses for JSON.
pub const SCHEMA_VERSION: u32 = 2;
const LEGACY_SCHEMA_VERSION: u32 = 1;

/// File-level key-value metadata keys. Every field here is
/// required on write — the file must be fully self-describing, because
/// the live instrument catalog it might otherwise be tempted to borrow
/// scales from is refetched every 24 hours and demonstrably changes
/// (BitMart, Phemex and Gate all misreported their increments
/// this project has already hit).
mod meta_key {
    pub(super) const SCHEMA_VERSION: &str = "senken.schema_version";
    pub(super) const SOURCE_ID: &str = "senken.source_id";
    pub(super) const SYMBOL: &str = "senken.symbol";
    pub(super) const ORIGIN: &str = "senken.origin";
    pub(super) const SPEC: &str = "senken.spec";
    pub(super) const PRICE_SCALE: &str = "senken.price_scale";
    pub(super) const QTY_SCALE: &str = "senken.qty_scale";
    pub(super) const PRICE_BASIS: &str = "senken.price_basis";
    pub(super) const RANGE_START: &str = "senken.range_start";
    pub(super) const RANGE_END: &str = "senken.range_end";
}

/// Everything a bar file's metadata must self-describe
/// : which series it belongs to, at what scale, and what range it
/// declares coverage for.
///
/// `anchor` is deliberately not here: it lives in the
/// *path*, not the file metadata, because coverage listing (`Store::coverage`,
/// no Arrow) must be able to tell two anchors of the same nominal spec
/// apart without opening a file — see `spec_token`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SeriesMetadata {
    pub(crate) schema_version: u32,
    /// The plugin's unit of registration, e.g. `binance-usdm`.
    pub source_id: String,
    /// The normalised symbol, e.g. `BTCUSDT`.
    pub symbol: String,
    /// Venue-supplied or locally aggregated.
    pub origin: Origin,
    /// The market value used to build OHLC prices.
    pub price_basis: BarPriceBasis,
    /// The timeframe.
    pub spec: BarSpec,
    /// Decimal places `open`/`high`/`low`/`close` are scaled by.
    pub price_scale: u8,
    /// Decimal places `volume`/`quote_volume`/`taker_buy_volume` are
    /// scaled by.
    pub qty_scale: u8,
    /// The range this file declares itself to cover — the
    /// same value [`crate::encode_range`] turns into the file's name, kept
    /// in the metadata too so the file is self-describing even if it were
    /// ever moved or renamed.
    pub range: TimeRange,
}

impl SeriesMetadata {
    /// Encodes this metadata as the key-value pairs written into the
    /// Parquet file footer.
    #[must_use]
    pub(crate) fn to_kv_metadata(&self) -> HashMap<String, String> {
        HashMap::from([
            (
                meta_key::SCHEMA_VERSION.to_owned(),
                self.schema_version.to_string(),
            ),
            (meta_key::SOURCE_ID.to_owned(), self.source_id.clone()),
            (meta_key::SYMBOL.to_owned(), self.symbol.clone()),
            (meta_key::ORIGIN.to_owned(), self.origin.to_string()),
            (
                meta_key::PRICE_BASIS.to_owned(),
                match self.price_basis {
                    BarPriceBasis::Trade => "trade",
                    BarPriceBasis::Bid => "bid",
                }
                .to_owned(),
            ),
            (meta_key::SPEC.to_owned(), self.spec.to_string()),
            (
                meta_key::PRICE_SCALE.to_owned(),
                self.price_scale.to_string(),
            ),
            (meta_key::QTY_SCALE.to_owned(), self.qty_scale.to_string()),
            (
                meta_key::RANGE_START.to_owned(),
                self.range.start().as_nanos().to_string(),
            ),
            (
                meta_key::RANGE_END.to_owned(),
                self.range.end().as_nanos().to_string(),
            ),
        ])
    }

    /// [`Self::to_kv_metadata`], shaped for
    /// [`parquet::file::properties::WriterPropertiesBuilder::set_key_value_metadata`].
    ///
    /// Written this way rather than relying on the Arrow schema's own
    /// metadata surviving the round trip: measured directly against this
    /// workspace's pinned `arrow`/`parquet` 59.2.0, a plain
    /// `Schema::new_with_metadata` attached only to the *writer's* input
    /// schema does **not** reappear on `RecordBatch::schema()` after a
    /// read back — the reconstructed schema comes back with empty
    /// metadata. Parquet's own file-level key-value metadata (the /// actual requirement) does not have that problem, so this crate
    /// writes there and reattaches it to each batch's schema on read
    /// (`reader.rs`) rather than depending on Arrow's own metadata
    /// plumbing.
    #[must_use]
    pub(crate) fn to_parquet_key_values(&self) -> Vec<parquet::file::metadata::KeyValue> {
        self.to_kv_metadata()
            .into_iter()
            .map(|(key, value)| parquet::file::metadata::KeyValue::new(key, value))
            .collect()
    }

    /// Decodes the metadata a Parquet/Arrow schema carries back into
    /// [`SeriesMetadata`] — the public counterpart to
    /// [`Store::read_range`](crate::Store::read_range): every returned
    /// `RecordBatch`'s [`RecordBatch::schema`](arrow::array::RecordBatch::schema)
    /// carries exactly this, so a caller can recover which series, scale
    /// and declared range a batch came from without re-deriving it from
    /// the path.
    ///
    /// # Errors
    /// A plain decode failure via [`arrow::error::ArrowError`] if a
    /// required key is missing, unparsable, or names an unsupported
    /// schema version — a corrupt or foreign-written footer is an
    /// Arrow-shaped concern, not one of this crate's own
    /// [`crate::WriteAssertionError`]s, which only apply to writes this
    /// crate itself performs.
    pub fn from_schema(schema: &Schema) -> Result<Self, arrow::error::ArrowError> {
        Self::from_kv_metadata(schema.metadata())
    }

    /// As [`Self::from_schema`], reading Parquet's own file-level
    /// key-value metadata directly (see [`Self::to_parquet_key_values`]
    /// for why writes land there rather than on the Arrow schema).
    pub(crate) fn from_parquet_key_values(
        kv: Option<&Vec<parquet::file::metadata::KeyValue>>,
    ) -> Result<Self, arrow::error::ArrowError> {
        let map: HashMap<String, String> = kv
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.value.clone().map(|value| (entry.key.clone(), value)))
            .collect();
        Self::from_kv_metadata(&map)
    }

    /// [`Self::from_schema`]'s implementation, kept separate so tests can
    /// exercise decode failures with a bare map instead of a full schema.
    fn from_kv_metadata(kv: &HashMap<String, String>) -> Result<Self, arrow::error::ArrowError> {
        fn get<'a>(
            kv: &'a HashMap<String, String>,
            key: &str,
        ) -> Result<&'a str, arrow::error::ArrowError> {
            kv.get(key)
                .map(String::as_str)
                .ok_or_else(|| arrow::error::ArrowError::SchemaError(format!("missing {key}")))
        }
        fn parse<T: FromStr>(
            kv: &HashMap<String, String>,
            key: &str,
        ) -> Result<T, arrow::error::ArrowError> {
            get(kv, key)?
                .parse()
                .map_err(|_| arrow::error::ArrowError::SchemaError(format!("invalid {key}")))
        }

        let schema_version: u32 = parse(kv, meta_key::SCHEMA_VERSION)?;
        if !matches!(schema_version, LEGACY_SCHEMA_VERSION | SCHEMA_VERSION) {
            return Err(arrow::error::ArrowError::SchemaError(format!(
                "unsupported senken.schema_version {schema_version} (expected 1 or {SCHEMA_VERSION})"
            )));
        }
        let range_start: i64 = parse(kv, meta_key::RANGE_START)?;
        let range_end: i64 = parse(kv, meta_key::RANGE_END)?;
        let range = TimeRange::new(
            UnixNanos::from_nanos(range_start),
            UnixNanos::from_nanos(range_end),
        )
        .ok_or_else(|| arrow::error::ArrowError::SchemaError("range end before start".into()))?;

        Ok(Self {
            schema_version,
            source_id: get(kv, meta_key::SOURCE_ID)?.to_owned(),
            symbol: get(kv, meta_key::SYMBOL)?.to_owned(),
            origin: parse(kv, meta_key::ORIGIN)?,
            price_basis: match kv.get(meta_key::PRICE_BASIS).map(String::as_str) {
                None | Some("trade") => BarPriceBasis::Trade,
                Some("bid") => BarPriceBasis::Bid,
                Some(_) => {
                    return Err(arrow::error::ArrowError::SchemaError(
                        "invalid senken.price_basis".to_owned(),
                    ));
                }
            },
            spec: parse(kv, meta_key::SPEC)?,
            price_scale: parse(kv, meta_key::PRICE_SCALE)?,
            qty_scale: parse(kv, meta_key::QTY_SCALE)?,
            range,
        })
    }
}

/// The nine bar columns, in a fixed order. Shared by
/// [`arrow_schema`] (write side) and `reader.rs` (which reattaches file
/// metadata to a schema built from these same fields on read).
fn bar_fields(schema_version: u32) -> Vec<Field> {
    let mut fields = vec![
        Field::new("ts_open", DataType::Int64, false),
        Field::new("open", DataType::Int64, false),
        Field::new("high", DataType::Int64, false),
        Field::new("low", DataType::Int64, false),
        Field::new("close", DataType::Int64, false),
        Field::new("volume", DataType::Int64, schema_version >= SCHEMA_VERSION),
        Field::new("quote_volume", DataType::Int64, true),
        Field::new("trade_count", DataType::UInt32, true),
        Field::new("taker_buy_volume", DataType::Int64, true),
    ];
    if schema_version >= SCHEMA_VERSION {
        fields.push(Field::new("volume_kind", DataType::UInt8, false));
    }
    fields
}

/// The Arrow schema for a bar file: eight columns, in a fixed
/// order, plus `metadata`'s key-value pairs attached at the schema level.
///
/// This schema is only used to *write*: see
/// [`SeriesMetadata::to_parquet_key_values`] for why the metadata attached
/// here does not reliably come back on read, and
/// [`schema_with_metadata`] for the read-side counterpart.
#[must_use]
pub(crate) fn arrow_schema(metadata: &SeriesMetadata) -> SchemaRef {
    let fields = bar_fields(SCHEMA_VERSION);
    Arc::new(Schema::new_with_metadata(fields, metadata.to_kv_metadata()))
}

/// Rebuilds a schema over the same nine bar columns with `kv` attached as
/// its metadata — the read-side counterpart to [`arrow_schema`], used to
/// reattach a file's Parquet-level metadata (read via
/// [`SeriesMetadata::from_parquet_key_values`]) onto each batch
/// (`reader.rs`), so a caller inspecting `RecordBatch::schema()` sees the
/// same metadata a writer declared, regardless of how Arrow's own schema
/// plumbing round-trips it.
#[must_use]
pub(crate) fn schema_with_metadata(metadata: &SeriesMetadata) -> SchemaRef {
    Arc::new(Schema::new_with_metadata(
        bar_fields(metadata.schema_version),
        metadata.to_kv_metadata(),
    ))
}
