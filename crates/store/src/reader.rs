//! [`Store::read_range`]: streams row groups from exactly the
//! files a query can touch, pruned by filename before any file is opened.

use std::collections::VecDeque;
use std::fs::File;
use std::path::PathBuf;

use arrow::array::{Array, Int64Array, RecordBatch, UInt32Array};
use arrow::datatypes::SchemaRef;
use parquet::arrow::arrow_reader::{ParquetRecordBatchReader, ParquetRecordBatchReaderBuilder};
use senken_core::{TimeRange, UnixNanos};
use senken_series::{Anchor, Bar, SeriesKey};

use crate::error::StoreError;
use crate::schema::{SeriesMetadata, schema_with_metadata};
use crate::store::{Store, list_range_entries};

impl Store {
    /// Streams every row whose bar might fall in `range`, row group by row
    /// group, across every file this series' declared coverage says can
    /// overlap it.
    ///
    /// Files are **pruned by filename before any of them is opened**
    ///   — a query wholly outside a file's declared range never
    /// touches that file at all — and even a file that *is* opened is
    /// read one row group at a time rather than loaded whole, so a
    /// multi-year series costs memory proportional to one row group, not
    /// to the series.
    ///
    /// The returned iterator yields Arrow `RecordBatch`es in whatever
    /// order the underlying files sort in by range (chronological, since
    /// [`crate::encode_range`] is lexicographically sortable); a caller
    /// wanting only rows strictly inside `range` still has to filter
    /// `ts_open`, since a returned batch's row group may itself span
    /// beyond the exact query boundary.
    ///
    /// # Errors
    /// [`StoreError::Io`] if the coverage directory cannot be listed;
    /// otherwise as file opens and Parquet reads fail, per batch.
    pub fn read_range(
        &self,
        key: &SeriesKey,
        anchor: Anchor,
        range: TimeRange,
    ) -> Result<impl Iterator<Item = Result<RecordBatch, StoreError>>, StoreError> {
        let dir = self.data_dir().join(crate::paths::bars_dir(key, anchor));
        let mut candidates = list_range_entries(&dir)?
            .into_iter()
            .filter(|(_name, file_range)| file_range.intersect(&range).is_some())
            .map(|(name, file_range)| (dir.join(name), file_range))
            .collect::<Vec<_>>();
        // Chronological order, so a caller consuming the stream in order
        // sees bars in ascending `ts_open`.
        candidates.sort_by_key(|(_path, r)| r.start());

        Ok(RangeReader {
            pending: candidates.into_iter().map(|(path, _range)| path).collect(),
            current: None,
            current_schema: None,
        })
    }
}

/// Lazily opens each pending file only when the previous one is exhausted,
/// delegating actual row-group-by-row-group streaming to
/// [`ParquetRecordBatchReader`], which never materialises a whole file.
struct RangeReader {
    pending: VecDeque<PathBuf>,
    current: Option<ParquetRecordBatchReader>,
    /// The current file's metadata, reattached to every batch it yields
    /// (see [`schema_with_metadata`]'s doc comment for why this is done
    /// by hand rather than trusted to Arrow's own schema round-trip).
    current_schema: Option<SchemaRef>,
}

impl Iterator for RangeReader {
    type Item = Result<RecordBatch, StoreError>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(reader) = self.current.as_mut()
                && let Some(batch) = reader.next()
            {
                let schema = self
                    .current_schema
                    .clone()
                    .expect("current_schema is always set alongside current");
                return Some(
                    batch
                        .map_err(StoreError::from)
                        .and_then(|batch| batch.with_schema(schema).map_err(StoreError::from)),
                );
            }
            self.current = None;
            self.current_schema = None;

            let path = self.pending.pop_front()?;
            let file = match File::open(&path) {
                Ok(file) => file,
                Err(source) => return Some(Err(StoreError::Io { path, source })),
            };
            let builder = match ParquetRecordBatchReaderBuilder::try_new(file) {
                Ok(builder) => builder,
                Err(e) => return Some(Err(StoreError::from(e))),
            };
            let file_metadata = builder.metadata().file_metadata().clone();
            let metadata =
                match SeriesMetadata::from_parquet_key_values(file_metadata.key_value_metadata()) {
                    Ok(metadata) => metadata,
                    Err(e) => return Some(Err(StoreError::from(e))),
                };
            self.current_schema = Some(schema_with_metadata(metadata.to_kv_metadata()));
            match builder.build() {
                Ok(reader) => self.current = Some(reader),
                Err(e) => return Some(Err(StoreError::from(e))),
            }
        }
    }
}

/// Decodes one `RecordBatch` (as yielded by [`Store::read_range`]) back into
/// plain [`Bar`]s, undoing exactly what `writer.rs`'s `encode_bars` built.
///
/// This exists so a consumer of this crate — `senken-loader` in
/// particular — never has to depend on `arrow` itself just to turn a read
/// back into bars. Arrow stays confined to `senken-store` (this crate's own
/// module docs): a caller gets `Vec<Bar>` in, `Vec<Bar>` out.
///
/// # Errors
/// [`StoreError::Arrow`] if `batch` does not have the nine bar columns in
/// this crate's fixed schema order and types — which cannot happen for a
/// batch this crate produced itself via [`Store::read_range`], but a caller
/// could in principle hand back an unrelated batch.
pub fn bars_from_batch(batch: &RecordBatch) -> Result<Vec<Bar>, StoreError> {
    fn i64_column<'a>(
        batch: &'a RecordBatch,
        index: usize,
        name: &str,
    ) -> Result<&'a Int64Array, StoreError> {
        batch
            .column(index)
            .as_any()
            .downcast_ref::<Int64Array>()
            .ok_or_else(|| {
                StoreError::from(arrow::error::ArrowError::SchemaError(format!(
                    "column {index} ({name}) is not Int64"
                )))
            })
    }

    let ts_open = i64_column(batch, 0, "ts_open")?;
    let open = i64_column(batch, 1, "open")?;
    let high = i64_column(batch, 2, "high")?;
    let low = i64_column(batch, 3, "low")?;
    let close = i64_column(batch, 4, "close")?;
    let volume = i64_column(batch, 5, "volume")?;
    let quote_volume = i64_column(batch, 6, "quote_volume")?;
    let trade_count = batch
        .column(7)
        .as_any()
        .downcast_ref::<UInt32Array>()
        .ok_or_else(|| {
            StoreError::from(arrow::error::ArrowError::SchemaError(
                "column 7 (trade_count) is not UInt32".to_owned(),
            ))
        })?;
    let taker_buy_volume = i64_column(batch, 8, "taker_buy_volume")?;

    Ok((0..batch.num_rows())
        .map(|row| Bar {
            ts_open: UnixNanos::from_nanos(ts_open.value(row)),
            open: open.value(row),
            high: high.value(row),
            low: low.value(row),
            close: close.value(row),
            volume: volume.value(row),
            quote_volume: (!quote_volume.is_null(row)).then(|| quote_volume.value(row)),
            trade_count: (!trade_count.is_null(row)).then(|| trade_count.value(row)),
            taker_buy_volume: (!taker_buy_volume.is_null(row)).then(|| taker_buy_volume.value(row)),
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Store;
    use arrow::array::{Array, Int64Array};
    use senken_core::UnixNanos;
    use senken_series::{Bar, BarSpec, BarUnit, Origin};
    use tempfile::TempDir;

    fn bar(ts_open_secs: i64) -> Bar {
        Bar {
            ts_open: UnixNanos::from_secs(ts_open_secs).unwrap(),
            open: 100,
            high: 110,
            low: 90,
            close: 105,
            volume: 1_000,
            quote_volume: Some(50_000),
            trade_count: Some(7),
            taker_buy_volume: Some(400),
        }
    }

    fn key() -> SeriesKey {
        SeriesKey::new(
            "binance-spot",
            "BTCUSDT",
            Origin::Venue,
            BarSpec::new(1, BarUnit::Minute),
        )
    }

    fn secs_range(start: i64, end: i64) -> TimeRange {
        TimeRange::new(
            UnixNanos::from_secs(start).unwrap(),
            UnixNanos::from_secs(end).unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn a_written_file_round_trips_byte_identical_rows_and_metadata() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let range = secs_range(0, 180);
        let bars = [bar(0), bar(60), bar(120)];
        store
            .write(&key(), Anchor::UTC, 2, 8, range, &bars)
            .unwrap();

        let batches: Vec<_> = store
            .read_range(&key(), Anchor::UTC, range)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(batches.len(), 1);
        let batch = &batches[0];
        assert_eq!(batch.num_rows(), 3);

        let ts_open = batch
            .column(0)
            .as_any()
            .downcast_ref::<Int64Array>()
            .unwrap();
        assert_eq!(ts_open.values(), &[0, 60_000_000_000, 120_000_000_000]);

        let metadata = crate::SeriesMetadata::from_schema(batch.schema_ref().as_ref()).unwrap();
        assert_eq!(metadata.source_id, "binance-spot");
        assert_eq!(metadata.symbol, "BTCUSDT");
        assert_eq!(metadata.price_scale, 2);
        assert_eq!(metadata.qty_scale, 8);
        assert_eq!(metadata.range, range);
    }

    #[test]
    fn read_range_prunes_files_outside_the_query_by_filename_alone() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let early = secs_range(0, 60);
        let late = secs_range(6_000_000, 6_000_060);
        store
            .write(&key(), Anchor::UTC, 2, 8, early, &[bar(0)])
            .unwrap();
        store
            .write(&key(), Anchor::UTC, 2, 8, late, &[bar(6_000_000)])
            .unwrap();

        // Corrupt the early file on disk; if `read_range` opened it while
        // querying only the late range, this would surface as an error.
        let bars_dir = dir
            .path()
            .join("sources/binance-spot/instruments/BTCUSDT/bars/venue-1m");
        let early_file = bars_dir.join(format!("{}.parquet", crate::range::encode_range(early)));
        std::fs::write(&early_file, b"not parquet, would fail to open").unwrap();

        let batches: Vec<_> = store
            .read_range(&key(), Anchor::UTC, late)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].num_rows(), 1);
    }

    #[test]
    fn nullable_columns_carry_null_not_zero() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let range = secs_range(0, 60);
        let mut b = bar(0);
        b.trade_count = None; // Bybit's case: no count at all.
        store.write(&key(), Anchor::UTC, 2, 8, range, &[b]).unwrap();

        let batches: Vec<_> = store
            .read_range(&key(), Anchor::UTC, range)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        let trade_count = batches[0]
            .column_by_name("trade_count")
            .unwrap()
            .as_any()
            .downcast_ref::<arrow::array::UInt32Array>()
            .unwrap();
        assert!(trade_count.is_null(0), "must be null, not 0");
    }

    #[test]
    fn bars_from_batch_undoes_the_write_exactly() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let range = secs_range(0, 180);
        let mut with_nulls = bar(120);
        with_nulls.trade_count = None;
        let bars = [bar(0), bar(60), with_nulls];
        store
            .write(&key(), Anchor::UTC, 2, 8, range, &bars)
            .unwrap();

        let batches: Vec<_> = store
            .read_range(&key(), Anchor::UTC, range)
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(batches.len(), 1);
        let decoded = bars_from_batch(&batches[0]).unwrap();
        assert_eq!(
            decoded, bars,
            "decoding a written batch must reproduce the exact bars given to write(), so senken-loader never needs arrow to consume this crate's output"
        );
    }
}
