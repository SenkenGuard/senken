//! [`Store::write`]: serialises a batch of bars to a new,
//! immutable Parquet file and supersedes whatever coverage it fully
//! contains — never rewriting an existing file in place.

use std::sync::Arc;

use arrow::array::{ArrayRef, Int64Array, RecordBatch, UInt32Array};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, Encoding, ZstdLevel};
use parquet::file::properties::{WriterProperties, WriterVersion};
use parquet::schema::types::ColumnPath;
use senken_core::TimeRange;
use senken_series::{Anchor, Bar, SeriesKey};

use crate::assertions::assert_bars_valid;
use crate::error::{StoreError, WriteAssertionError};
use crate::paths::bars_file;
use crate::range::encode_range;
use crate::schema::{SeriesMetadata, arrow_schema};
use crate::store::{Store, list_range_entries};

/// Row group size for M1 bars: roughly one day of one-minute bars
/// , so a reader streaming row-group by row-group never holds much
/// more than a day's worth of one series in memory at once.
const ROWS_PER_ROW_GROUP: usize = 1_440;

impl Store {
    /// Writes `bars` as a new Parquet file declaring coverage of `range`
    /// (the filename states what was *fetched*, which may be
    /// wider than what `bars` actually contains — a gap inside `range`
    /// with no row is a real market gap, never synthesised).
    ///
    /// If `range` fully contains one or more of this series' existing
    /// files, those files are unlinked once the new file is durably in
    /// place (extend by writing new + unlinking old, never by
    /// rewriting in place). A file only *partially* overlapping `range` —
    /// which a correctly gap-planned caller should never produce — is
    /// treated as a conflict and rejected, since accepting it would leave
    /// two overlapping files with no way to tell a reader which one to
    /// trust for the shared span.
    ///
    /// # Errors
    /// [`StoreError::Rejected`] if `bars` fails any M5.3 assertion, or if
    /// the range would partially overlap an existing file
    /// ([`WriteAssertionError::OverlapsExistingCoverage`]); otherwise
    /// [`StoreError::Storage`]/[`StoreError::Parquet`]/[`StoreError::Arrow`]
    /// as the write itself fails.
    pub fn write(
        &self,
        key: &SeriesKey,
        anchor: Anchor,
        price_scale: u8,
        qty_scale: u8,
        range: TimeRange,
        bars: &[Bar],
    ) -> Result<(), StoreError> {
        assert_bars_valid(bars, key.spec, anchor, range)?;

        let dir = self.data_dir().join(crate::paths::bars_dir(key, anchor));
        let existing = list_range_entries(&dir)?;

        let mut superseded = Vec::new();
        for (name, existing_range) in existing {
            // A filename is a pure function of its range, so a file covering
            // exactly this range *is* the file this write is about to
            // produce. Collecting it as superseded would unlink the new data
            // moments after writing it — silently, and with nothing left to
            // recover from. Two concurrent backfills of one series reach this
            // state legitimately, so it is skipped rather than assumed
            // impossible.
            if existing_range.start() == range.start() && existing_range.end() == range.end() {
                continue;
            }
            if existing_range.intersect(&range).is_none() {
                continue; // disjoint — untouched by this write
            }
            let fully_contained =
                existing_range.start() >= range.start() && existing_range.end() <= range.end();
            if !fully_contained {
                return Err(WriteAssertionError::OverlapsExistingCoverage {
                    new_start: range.start(),
                    new_end: range.end(),
                    existing_start: existing_range.start(),
                    existing_end: existing_range.end(),
                }
                .into());
            }
            superseded.push(dir.join(name));
        }

        let metadata = SeriesMetadata {
            source_id: key.source_id.to_string(),
            symbol: key.symbol.to_string(),
            origin: key.origin,
            spec: key.spec,
            price_scale,
            qty_scale,
            range,
        };
        let bytes = encode_bars(bars, &metadata)?;

        let rel_path = bars_file(key, anchor, &encode_range(range));
        self.storage().write_bytes(&rel_path, &bytes)?;

        // Only unlink superseded files once the new one is durably
        // written and renamed into place — a crash before this point
        // leaves the old coverage intact and simply loses the new write,
        // never the other way around.
        for old_path in superseded {
            let old_rel = old_path.strip_prefix(self.data_dir()).unwrap_or(&old_path);
            self.storage().remove(old_rel)?;
        }

        Ok(())
    }
}

/// Builds the Arrow `RecordBatch` for `bars` and serialises it to a Parquet
/// byte buffer with `metadata` attached at the schema level.
fn encode_bars(bars: &[Bar], metadata: &SeriesMetadata) -> Result<Vec<u8>, StoreError> {
    let schema = arrow_schema(metadata);

    let ts_open: ArrayRef = Arc::new(Int64Array::from_iter_values(
        bars.iter().map(|b| b.ts_open.as_nanos()),
    ));
    let open: ArrayRef = Arc::new(Int64Array::from_iter_values(bars.iter().map(|b| b.open)));
    let high: ArrayRef = Arc::new(Int64Array::from_iter_values(bars.iter().map(|b| b.high)));
    let low: ArrayRef = Arc::new(Int64Array::from_iter_values(bars.iter().map(|b| b.low)));
    let close: ArrayRef = Arc::new(Int64Array::from_iter_values(bars.iter().map(|b| b.close)));
    let volume: ArrayRef = Arc::new(Int64Array::from_iter_values(bars.iter().map(|b| b.volume)));
    // Nullable columns are built from `Option`s directly — a `None` here
    // becomes a genuine Parquet null, never a `0` (Bybit
    // reports no trade count at all, and a `0` would claim "no trades",
    // which is a different, false, statement).
    let quote_volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| b.quote_volume).collect::<Vec<_>>(),
    ));
    let trade_count: ArrayRef = Arc::new(UInt32Array::from(
        bars.iter().map(|b| b.trade_count).collect::<Vec<_>>(),
    ));
    let taker_buy_volume: ArrayRef = Arc::new(Int64Array::from(
        bars.iter().map(|b| b.taker_buy_volume).collect::<Vec<_>>(),
    ));

    let batch = RecordBatch::try_new(
        Arc::clone(&schema),
        vec![
            ts_open,
            open,
            high,
            low,
            close,
            volume,
            quote_volume,
            trade_count,
            taker_buy_volume,
        ],
    )?;

    let props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_2_0)
        .set_compression(Compression::ZSTD(ZstdLevel::default()))
        .set_max_row_group_row_count(Some(ROWS_PER_ROW_GROUP))
        // `ts_open` is strictly increasing and delta-friendly —
        // successive opens differ by a constant interval far more often
        // than not.
        .set_column_encoding(ColumnPath::from("ts_open"), Encoding::DELTA_BINARY_PACKED)
        // Parquet's own file-level key-value metadata (the actual
        // requirement) — see `to_parquet_key_values`'s doc comment for
        // why this crate does not rely on the Arrow schema's metadata
        // instead.
        .set_key_value_metadata(Some(metadata.to_parquet_key_values()))
        .build();

    let mut buf: Vec<u8> = Vec::new();
    let mut writer = ArrowWriter::try_new(&mut buf, schema, Some(props))?;
    writer.write(&batch)?;
    writer.close()?;
    Ok(buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use senken_core::UnixNanos;
    use senken_series::{BarSpec, BarUnit, Origin};
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
            trade_count: None,
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

    #[test]
    fn write_rejects_an_invalid_batch_without_touching_disk() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let range = TimeRange::new(
            UnixNanos::from_secs(0).unwrap(),
            UnixNanos::from_secs(120).unwrap(),
        )
        .unwrap();
        // Misaligned: 30 seconds into a one-minute bar.
        let bars = [bar(30)];
        let err = store
            .write(&key(), Anchor::UTC, 2, 8, range, &bars)
            .unwrap_err();
        assert!(matches!(err, StoreError::Rejected(_)));
        assert!(
            store.coverage(&key(), Anchor::UTC).unwrap().is_empty(),
            "a rejected write must leave no file behind"
        );
    }

    #[test]
    fn rewriting_an_identical_range_keeps_the_data_instead_of_deleting_it() {
        // A filename is a pure function of its range, so this rewrite lands
        // on the same path. Collecting that path as "superseded" would unlink
        // the bars moments after writing them — silently. Two concurrent
        // backfills of one series can legitimately reach this state.
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();
        let range = TimeRange::new(
            UnixNanos::from_secs(0).unwrap(),
            UnixNanos::from_secs(120).unwrap(),
        )
        .unwrap();
        let bars = [bar(0), bar(60)];

        store
            .write(&key(), Anchor::UTC, 2, 8, range, &bars)
            .unwrap();
        store
            .write(&key(), Anchor::UTC, 2, 8, range, &bars)
            .unwrap();

        assert_eq!(
            store.coverage(&key(), Anchor::UTC).unwrap().len(),
            1,
            "the rewrite must leave exactly one file, not zero"
        );
        let read: Vec<_> = store
            .read_range(&key(), Anchor::UTC, range)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let total: usize = read.iter().map(arrow::array::RecordBatch::num_rows).sum();
        assert_eq!(total, 2, "both bars must survive a same-range rewrite");
    }

    #[test]
    fn extending_coverage_writes_a_new_file_and_removes_the_old() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let first_range = TimeRange::new(
            UnixNanos::from_secs(0).unwrap(),
            UnixNanos::from_secs(120).unwrap(),
        )
        .unwrap();
        store
            .write(&key(), Anchor::UTC, 2, 8, first_range, &[bar(0), bar(60)])
            .unwrap();
        assert_eq!(
            store.coverage(&key(), Anchor::UTC).unwrap(),
            vec![first_range]
        );

        let extended_range = TimeRange::new(
            UnixNanos::from_secs(0).unwrap(),
            UnixNanos::from_secs(180).unwrap(),
        )
        .unwrap();
        store
            .write(
                &key(),
                Anchor::UTC,
                2,
                8,
                extended_range,
                &[bar(0), bar(60), bar(120)],
            )
            .unwrap();

        let coverage = store.coverage(&key(), Anchor::UTC).unwrap();
        assert_eq!(
            coverage,
            vec![extended_range],
            "the old file must be gone, leaving only the extended one — no in-place rewrite"
        );
    }

    #[test]
    fn a_partial_overlap_with_an_existing_file_is_rejected() {
        let dir = TempDir::new().unwrap();
        let store = Store::new(dir.path());
        store.init().unwrap();

        let first_range = TimeRange::new(
            UnixNanos::from_secs(0).unwrap(),
            UnixNanos::from_secs(120).unwrap(),
        )
        .unwrap();
        store
            .write(&key(), Anchor::UTC, 2, 8, first_range, &[bar(0), bar(60)])
            .unwrap();

        // [60, 180) overlaps [0, 120) without containing it.
        let overlapping_range = TimeRange::new(
            UnixNanos::from_secs(60).unwrap(),
            UnixNanos::from_secs(180).unwrap(),
        )
        .unwrap();
        let err = store
            .write(
                &key(),
                Anchor::UTC,
                2,
                8,
                overlapping_range,
                &[bar(60), bar(120)],
            )
            .unwrap_err();
        assert!(matches!(
            err,
            StoreError::Rejected(WriteAssertionError::OverlapsExistingCoverage { .. })
        ));
        // The first file must still be exactly as it was.
        assert_eq!(
            store.coverage(&key(), Anchor::UTC).unwrap(),
            vec![first_range]
        );
    }
}
