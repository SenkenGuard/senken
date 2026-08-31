//! `cargo bench -p senken-marketdata`
//!
//! Measures the catalog hot paths in isolation and the full
//! `MarketData::instruments` pipeline against warm 5k-instrument sources.
//! `AllocProfiler` adds allocation counts to every row: the search pipeline
//! is judged on allocations as much as on wall time.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use chrono::Utc;
use divan::{AllocProfiler, Bencher, black_box};
use senken_marketdata::MarketData;
use senken_marketdata::catalog::SourceCatalog;
use senken_marketdata::instrument::{Instrument, InstrumentStatus};
use senken_marketdata::query::InstrumentQuery;
use senken_marketdata::source::{MarketDataSource, SourceError};
use senken_storage::Storage;

#[global_allocator]
static ALLOC: AllocProfiler = AllocProfiler::system();

const COUNT: usize = 5_000;
const QUOTES: [&str; 8] = ["USDT", "USDC", "BTC", "ETH", "TRY", "EUR", "BRL", "AUD"];

fn main() {
    divan::main();
}

fn synthetic() -> &'static [Instrument] {
    static INSTRUMENTS: OnceLock<Vec<Instrument>> = OnceLock::new();
    INSTRUMENTS.get_or_init(|| {
        (0..COUNT)
            .map(|i| {
                let base = format!("A{i:04}");
                let quote = QUOTES[i % QUOTES.len()];
                Instrument::spot(
                    format!("{base}{quote}"),
                    format!("{base}-{quote}"),
                    base.as_str(),
                    quote,
                )
                .with_status(InstrumentStatus::Trading)
                .with_price_increment((8, 1))
                .with_qty_increment((8, 1))
            })
            .collect()
    })
}

fn catalog() -> &'static SourceCatalog {
    static CATALOG: OnceLock<SourceCatalog> = OnceLock::new();
    CATALOG.get_or_init(|| {
        SourceCatalog::new("bench", "Bench Venue", Utc::now(), synthetic().to_vec())
    })
}

#[divan::bench]
fn scan_and_match(b: Bencher<'_, '_>) {
    let catalog = catalog();
    let query = InstrumentQuery::new("A0042");
    let source_matches = query.matches_source("bench", "Bench Venue");
    b.bench(|| {
        catalog
            .instruments()
            .iter()
            .filter(|i| query.rank(i, source_matches).is_some())
            .count()
    });
}

#[divan::bench]
fn exact_lookup_indexed(b: Bencher<'_, '_>) {
    let catalog = catalog();
    b.bench(|| catalog.find(black_box("a0042usdt")));
}

#[divan::bench]
fn exact_lookup_linear_scan(b: Bencher<'_, '_>) {
    let catalog = catalog();
    b.bench(|| {
        catalog
            .instruments()
            .iter()
            .find(|i| i.symbol.eq_ignore_ascii_case(black_box("A0042USDT")))
    });
}

#[divan::bench]
fn deserialise_snapshot(b: Bencher<'_, '_>) {
    let json = serde_json::to_vec(synthetic()).unwrap();
    b.bench(|| serde_json::from_slice::<Vec<Instrument>>(&json).unwrap());
}

#[divan::bench]
fn build_catalog_index(b: Bencher<'_, '_>) {
    b.with_inputs(|| synthetic().to_vec())
        .bench_values(|instruments| {
            SourceCatalog::new("bench", "Bench Venue", Utc::now(), instruments)
        });
}

struct NamedSource {
    id: &'static str,
    name: &'static str,
}

#[async_trait]
impl MarketDataSource for NamedSource {
    fn id(&self) -> &str {
        self.id
    }

    fn name(&self) -> &str {
        self.name
    }

    async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
        Ok(synthetic().to_vec())
    }
}

fn warm_market_data(
    sources: &[(&'static str, &'static str)],
) -> (tokio::runtime::Runtime, tempfile::TempDir, MarketData) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let storage = Storage::new(dir.path());
    storage.init().unwrap();
    let mut md = MarketData::new(Arc::new(storage));
    for &(id, name) in sources {
        md.register_source(Arc::new(NamedSource { id, name }))
            .unwrap();
    }
    runtime.block_on(md.instruments("")); // warm the catalogs
    (runtime, dir, md)
}

/// The real entry point: match, rank, sort, interleave, paginate.
#[divan::bench(args = ["A0042", "usdt", ""])]
fn end_to_end_search(b: Bencher<'_, '_>, term: &str) {
    let (runtime, _dir, md) = warm_market_data(&[("bench", "Bench Venue")]);
    b.bench(|| {
        runtime
            .block_on(md.instruments(InstrumentQuery::new(term).with_limit(20)))
            .total_matched
    });
}

/// Four venues listing the same instruments: exercises the interleave path.
#[divan::bench(args = ["A0042", "usdt", ""])]
fn end_to_end_search_four_sources(b: Bencher<'_, '_>, term: &str) {
    let (runtime, _dir, md) = warm_market_data(&[
        ("bench-a", "Venue A"),
        ("bench-b", "Venue B"),
        ("bench-c", "Venue C"),
        ("bench-d", "Venue D"),
    ]);
    b.bench(|| {
        runtime
            .block_on(md.instruments(InstrumentQuery::new(term).with_limit(20)))
            .total_matched
    });
}
