//! Proves a plugin *package*'s `venue` contribution reaches
//! `Runtime::build()` end to end: install a package carrying the same
//! recorded-OKX fixture `senken-plugin-host`'s own tests prove the
//! lower-level bridge with, then build a real `Runtime` pointed at that
//! data directory and check its `MarketData`/`SeriesLoader` catalogs, not a
//! description of what installing a package would do.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use senken_plugin::widget_package::WidgetPackageStore;
use senken_runtime::Runtime;
use zip::write::SimpleFileOptions;

static BUILD_LOCK: Mutex<()> = Mutex::new(());

/// Builds `senken-plugin-host`'s own `fixture-venue-example` test fixture
/// and returns the path to its compiled component. Duplicates (rather than
/// depends on) that crate's private test helper — see
/// `senken_runtime::plugin_host`'s own copy of this same function for why.
fn build_venue_example_fixture() -> PathBuf {
    let _guard = BUILD_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let fixture_dir =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../plugin-host/tests/fixtures/venue-example");
    let shared_target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixture-wasm");
    let status = std::process::Command::new(env!("CARGO"))
        .args(["build", "--target", "wasm32-wasip2"])
        .env("CARGO_TARGET_DIR", &shared_target)
        .current_dir(&fixture_dir)
        .status()
        .expect("spawning `cargo build` for the venue-example fixture must succeed");
    assert!(status.success(), "fixture venue-example failed to build");
    let wasm_path = shared_target
        .join("wasm32-wasip2/debug")
        .join("fixture_venue_example.wasm");
    assert!(
        wasm_path.is_file(),
        "expected {} to exist after building the venue-example fixture",
        wasm_path.display()
    );
    wasm_path
}

/// OKX's own recorded responses, reused unchanged from
/// `senken_runtime::plugin_host`'s own `dynamic_venues_tests` module, which
/// proves the same fixture one layer down (against `DynamicVenues`
/// directly, with no package or `Runtime` involved).
const INSTRUMENTS: &[u8] = include_bytes!("../../../plugins/okx/tests/fixtures/instruments.json");
const CANDLES_1M: &[u8] = include_bytes!("../../../plugins/okx/tests/fixtures/candles_1m.json");

async fn mock_okx_server() -> wiremock::MockServer {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, ResponseTemplate};

    let server = wiremock::MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v5/public/instruments"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(INSTRUMENTS, "application/json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v5/market/history-candles"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES_1M, "application/json"))
        .mount(&server)
        .await;
    server
}

fn zip_venue_package(base_url: &str, wasm: &[u8]) -> Vec<u8> {
    let manifest = format!(
        r#"{{
            "id": "example-venue-package",
            "name": "Example Venue Package",
            "version": "1.0.0",
            "description": "a packaged venue for tests",
            "contributes": [
                {{
                    "point": "venue",
                    "venue": {{ "entry": "venue.wasm", "base_url": "{base_url}" }}
                }}
            ]
        }}"#
    );
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file("manifest.json", options).unwrap();
        writer.write_all(manifest.as_bytes()).unwrap();
        writer.start_file("venue.wasm", options).unwrap();
        writer.write_all(wasm).unwrap();
        writer.finish().unwrap();
    }
    buffer.into_inner()
}

fn zip_broken_venue_package() -> Vec<u8> {
    let manifest = r#"{
        "id": "broken-venue-package",
        "name": "Broken Venue Package",
        "version": "1.0.0",
        "contributes": [
            { "point": "venue", "venue": { "entry": "venue.wasm" } }
        ]
    }"#;
    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file("manifest.json", options).unwrap();
        writer.write_all(manifest.as_bytes()).unwrap();
        writer.start_file("venue.wasm", options).unwrap();
        writer.write_all(b"not a real component").unwrap();
        writer.finish().unwrap();
    }
    buffer.into_inner()
}

#[tokio::test]
async fn a_package_with_a_venue_contribution_registers_a_dynamic_venue_whose_instruments_appear_in_the_catalog()
 {
    let server = mock_okx_server().await;
    let wasm = std::fs::read(build_venue_example_fixture()).unwrap();
    let data_dir = tempfile::tempdir().unwrap();

    let packages = WidgetPackageStore::open(data_dir.path()).unwrap();
    packages
        .install(&zip_venue_package(&server.uri(), &wasm))
        .unwrap();

    let runtime = Runtime::builder()
        .data_dir(data_dir.path())
        .build()
        .expect("a runtime with one valid venue package must still build");

    let sources = runtime.marketdata().sources();
    assert!(
        sources.iter().any(|s| s.id == "example-okx"),
        "the package's venue contribution must reach the same MarketData catalog a static plugin uses"
    );
    let page = runtime
        .marketdata()
        .instruments(senken_marketdata::InstrumentQuery::new("").with_source("example-okx"))
        .await;
    assert!(
        page.matches
            .iter()
            .any(|m| m.instrument.symbol == "BTCUSDT"),
        "the fixture's recorded instrument must survive the package -> Runtime path"
    );

    let catalog = runtime.plugin_catalog();
    let listing = catalog
        .iter()
        .find(|p| p.id == "example-okx")
        .expect("the loaded venue must appear in the unified plugin catalog");
    assert!(
        listing
            .contributes
            .contains(&senken_plugin::ContributionKind::Venue)
    );
}

#[tokio::test]
async fn disabling_the_package_empties_its_instruments_but_keeps_its_bar_files_readable() {
    let server = mock_okx_server().await;
    let wasm = std::fs::read(build_venue_example_fixture()).unwrap();
    let data_dir = tempfile::tempdir().unwrap();

    let packages = WidgetPackageStore::open(data_dir.path()).unwrap();
    packages
        .install(&zip_venue_package(&server.uri(), &wasm))
        .unwrap();

    let runtime = Runtime::builder()
        .data_dir(data_dir.path())
        .build()
        .unwrap();

    // Write one Parquet bar file through the same series store a real bar
    // fetch would use, before the venue is ever disabled — the retention
    // property this proves is that this file survives disabling, not that
    // disabling merely fails to delete a directory that was already empty.
    let key = senken_series::SeriesKey::new(
        "example-okx",
        "BTCUSDT",
        senken_series::Origin::Venue,
        senken_series::BarSpec::new(1, senken_series::BarUnit::Minute),
    );
    let range = senken_core::TimeRange::new(
        senken_core::UnixNanos::from_secs(0).unwrap(),
        senken_core::UnixNanos::from_secs(60).unwrap(),
    )
    .unwrap();
    let bar = senken_series::Bar {
        ts_open: senken_core::UnixNanos::from_secs(0).unwrap(),
        open: 1,
        high: 1,
        low: 1,
        close: 1,
        volume: senken_series::Volume::Real(1),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    };
    runtime
        .store()
        .write(&key, senken_series::Anchor::UTC, 2, 8, range, &[bar])
        .expect("writing a bar file before disabling must succeed");
    assert!(
        !runtime
            .store()
            .coverage(&key, senken_series::Anchor::UTC)
            .unwrap()
            .is_empty(),
        "the bar file must exist before the venue is disabled"
    );

    runtime
        .dynamic_venues()
        .set_enabled("example-okx", false)
        .unwrap();

    assert!(
        runtime
            .marketdata()
            .sources()
            .iter()
            .any(|s| s.id == "example-okx"),
        "a disabled venue must stay registered, reporting an empty catalog"
    );
    let page = runtime
        .marketdata()
        .instruments(senken_marketdata::InstrumentQuery::new("").with_source("example-okx"))
        .await;
    assert!(
        page.matches.is_empty(),
        "a disabled venue's instrument catalog must be empty"
    );
    assert!(
        !runtime
            .store()
            .coverage(&key, senken_series::Anchor::UTC)
            .unwrap()
            .is_empty(),
        "market data already downloaded must survive disabling the venue that sourced it"
    );
}

#[tokio::test]
async fn a_broken_package_does_not_prevent_the_runtime_from_building() {
    let data_dir = tempfile::tempdir().unwrap();
    let packages = WidgetPackageStore::open(data_dir.path()).unwrap();
    packages.install(&zip_broken_venue_package()).unwrap();

    let runtime = Runtime::builder()
        .data_dir(data_dir.path())
        .build()
        .expect("one broken venue package must never abort startup");

    let catalog = runtime.plugin_catalog();
    let broken = catalog
        .iter()
        .find(|p| {
            p.contributes
                .contains(&senken_plugin::ContributionKind::Venue)
                && p.state != senken_runtime::PluginListingState::Active
        })
        .expect("the broken package's component must be recorded as a failed catalog entry");
    assert!(matches!(
        broken.state,
        senken_runtime::PluginListingState::Failed(_)
    ));
}
