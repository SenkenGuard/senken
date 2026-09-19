//! Proves `wasm/`'s `bybit-venue` component — the one that will actually
//! run in production once Bybit is enabled — returns **exactly** what the
//! native adapter already returns for the same recorded bytes.
//!
//! Mirrors `plugins/okx/tests/wasm_parity.rs` exactly, the pattern the
//! whole porting wave exists to have: every venue after OKX and Bybit
//! copies it, so a wrong constant, a diverging normalisation rule, or a
//! rescaled price is caught here rather than trusted on the strength of
//! "it compiled".
//!
//! Both sides answer from the very same `wiremock` server, serving
//! `plugins/bybit/tests/fixtures/spot.json` and `kline_1m.json` verbatim —
//! the exact fixtures `plugins/bybit/src/{lib,bars}.rs`'s own unit tests
//! already decode, so this test adds no new recorded data of its own, only
//! a third caller comparing the first two.
//!
//! Unlike OKX's recorded `instruments.json`, this venue's recorded
//! `spot.json` carries a single, already-`"Trading"` row — no suspended
//! instrument to prove the wasm side omits. That omission (a non-`Trading`
//! row has no status field to carry across the wasm boundary and so is
//! dropped outright) is proven instead at the unit level, in
//! `bybit-core`'s own `a_non_trading_status_is_omitted_not_merely_unflagged`
//! test, against a fixture-derived field shape rather than this crate's
//! one real recorded document.

mod support;

use bybit_core::normalise_bybit_symbol;
use senken_core::{TimeRange, UnixNanos};
use senken_marketdata::source::MarketDataSource;
use senken_marketdata::{Instrument, SourceSymbol};
use senken_plugin::BarSource;
use senken_plugin_host::{PluginHost, PluginLimits};
use senken_venue::{LimitGroup, VenueClient};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SPOT: &[u8] = include_bytes!("fixtures/spot.json");
const KLINE: &[u8] = include_bytes!("fixtures/kline_1m.json");

fn test_client(name: &str) -> VenueClient {
    VenueClient::new(reqwest::Client::new(), LimitGroup::new(name))
}

/// Bybit's own wire format happens to equal its normalised symbol (both
/// `BTCUSDT`), unlike OKX's dashed `instId` — see
/// `plugins/bybit/src/bars.rs`'s own `btcusdt` test helper for the same
/// note.
fn btc_usdt() -> SourceSymbol {
    Instrument::spot("BTCUSDT", "BTCUSDT", "BTC", "USDT").source_symbol()
}

fn wide_range() -> TimeRange {
    TimeRange::new(
        UnixNanos::EPOCH,
        UnixNanos::from_millis(4_102_444_800_000).unwrap(),
    )
    .unwrap()
}

async fn mock_bybit_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/instruments-info"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(SPOT, "application/json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/v5/market/kline"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(KLINE, "application/json"))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn wasm_instruments_are_identical_to_native_for_the_recorded_fixture() {
    let server = mock_bybit_server().await;

    let native = senken_plugin_bybit::spot_source(test_client("native"))
        .with_url(format!("{}/v5/market/instruments-info", server.uri()))
        .instruments()
        .await
        .expect("the native spot source must decode the recorded fixture");

    let wasm = std::fs::read(support::build_bybit_venue()).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm"), Some(server.uri()))
        .expect("bybit-venue must load");
    // The source id is half of every instrument id a saved chart layout
    // stores, so the component has to serve spot under the very id the
    // compiled-in adapter did. Nothing else in this suite would notice it
    // changing: the rows would still match field for field, under an id no
    // saved layout and no live feed knows.
    assert_eq!(
        loaded.descriptor().id,
        senken_plugin_bybit::SPOT_ID,
        "the component must keep serving spot under the native source id"
    );

    let wasm_instruments = loaded
        .instruments()
        .expect("bybit-venue must decode the recorded fixture");

    assert!(
        !native.is_empty() && !wasm_instruments.is_empty(),
        "the fixture must actually produce instruments on both sides, or this test proves nothing"
    );
    assert_eq!(
        native.len(),
        wasm_instruments.len(),
        "native: {native:#?}\nwasm: {wasm_instruments:#?}"
    );

    for expected in &native {
        let actual = wasm_instruments
            .iter()
            .find(|i| i.symbol == expected.symbol)
            .unwrap_or_else(|| {
                panic!(
                    "{} present natively but missing from bybit-venue",
                    expected.symbol
                )
            });
        assert_eq!(actual.source_symbol, expected.source_symbol);
        assert_eq!(actual.name, expected.name);
        assert_eq!(actual.base, expected.base);
        assert_eq!(actual.quote, expected.quote);
        assert_eq!(actual.price_scale, expected.price_scale);
        assert_eq!(actual.tick_size, expected.tick_size);
        assert_eq!(actual.qty_scale, expected.qty_scale);
        assert_eq!(actual.step_size, expected.step_size);
    }
}

#[tokio::test]
async fn wasm_bars_are_identical_to_native_for_the_recorded_fixture() {
    let server = mock_bybit_server().await;

    let native = senken_plugin_bybit::bar_source_spot(test_client("native"))
        .with_url(format!("{}/v5/market/kline", server.uri()))
        .bars(
            &btc_usdt(),
            senken_series::BarSpec::new(1, senken_series::BarUnit::Minute),
            wide_range(),
        )
        .await
        .expect("the native bar source must decode the recorded fixture");

    let wasm = std::fs::read(support::build_bybit_venue()).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm"), Some(server.uri()))
        .expect("bybit-venue must load");
    let wasm_bars = loaded
        .bars(
            "BTCUSDT",
            senken_plugin_host::BarSpec {
                step: 1,
                unit: senken_plugin_host::BarUnit::Minute,
            },
            0,
            i64::MAX,
        )
        .expect("bybit-venue must decode the recorded fixture");

    assert!(
        !native.is_empty() && !wasm_bars.is_empty(),
        "the fixture must actually produce bars on both sides, or this test proves nothing"
    );
    assert_eq!(native.len(), wasm_bars.len());

    for (expected, actual) in native.iter().zip(wasm_bars.iter()) {
        // `senken_series::Bar`'s own price/quantity fields carry no scale
        // of their own — only the wasm side's WIT `bar` does, per that
        // record's own doc comment. `1` is this exact fixture's own known
        // price scale (`plugins/bybit/src/bars.rs`'s own
        // `fixture_rows_decode_…` test asserts the same shape for the same
        // bytes), so an equality here on top of the raw values below still
        // catches a rescale that happened to keep every raw integer the
        // same but changed what it means.
        assert_eq!(actual.open.scale, 1);
        assert_eq!(actual.open.scale, actual.high.scale);
        assert_eq!(actual.open.scale, actual.low.scale);
        assert_eq!(actual.open.scale, actual.close.scale);

        assert_eq!(actual.ts_open, expected.ts_open.as_nanos());
        assert_eq!(actual.open.value, expected.open);
        assert_eq!(actual.high.value, expected.high);
        assert_eq!(actual.low.value, expected.low);
        assert_eq!(actual.close.value, expected.close);
        let senken_plugin_host::Volume::Real(actual_volume) = actual.volume else {
            panic!("Bybit always reports real volume");
        };
        let senken_series::Volume::Real(expected_volume) = expected.volume else {
            panic!("Bybit always reports real volume");
        };
        assert_eq!(actual_volume.value, expected_volume);
        assert_eq!(actual.quote_volume.map(|q| q.value), expected.quote_volume);
        assert_eq!(
            expected.trade_count, None,
            "Bybit never reports a trade count: None, never a false 0"
        );
    }
}

/// The property `AGENTS.md` names directly: a venue's symbol is
/// normalised **once**, by one rule. The wasm component's own instrument
/// catalog (loaded through the real `PluginHost`, from the real recorded
/// fixture) is checked here against `bybit_core::normalise_bybit_symbol`
/// called directly on each row's own symbol — the same function
/// `plugins/bybit/src/lib.rs`'s native catalog builder and
/// `plugins/bybit/src/feed.rs`'s live decoder both call (see that
/// module's own
/// `the_live_decoder_normalises_a_dashed_option_symbol_the_same_way_the_catalog_does`
/// unit test for the other call site this integration test cannot reach —
/// it is crate-private). If any call site ever computed its own separator
/// rule instead of calling this function, this assertion would not itself
/// catch it (each call site is checked against the same function it
/// calls), but together with that unit test it pins both call sites to
/// identical output for every market shape this fixture carries.
#[tokio::test]
async fn the_wasm_catalogs_symbols_match_normalise_bybit_symbol_for_every_row() {
    let server = mock_bybit_server().await;
    let wasm = std::fs::read(support::build_bybit_venue()).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm-symbols"), Some(server.uri()))
        .expect("bybit-venue must load");
    let instruments = loaded
        .instruments()
        .expect("bybit-venue must decode the recorded fixture");

    assert!(!instruments.is_empty());
    for instrument in &instruments {
        assert_eq!(
            instrument.symbol,
            normalise_bybit_symbol(&instrument.source_symbol),
            "{} did not normalise the way normalise_bybit_symbol does",
            instrument.source_symbol
        );
    }
}

/// The recorded catalogue fixture is a single page, so every test above
/// exercises the complete-catalogue path. This one covers the other branch:
/// a venue that says there is more. Serving the first page as if it were
/// everything would look identical to a venue with few pairs, and nothing
/// downstream could tell the difference — so the component refuses instead.
///
/// The body here is not a recorded response: it is the shape the native
/// adapter's own paging test already uses for "one more page follows"
/// (`plugins/bybit/src/lib.rs`'s cursor test), trimmed to the two fields
/// this assertion is about.
#[tokio::test]
async fn a_catalogue_that_spans_more_than_one_page_is_refused_rather_than_truncated() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/v5/market/instruments-info"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(
                br#"{"retCode":0,"retMsg":"OK","result":{"list":[],"nextPageCursor":"page%3D2"}}"#
                    .to_vec(),
                "application/json",
            ),
        )
        .mount(&server)
        .await;

    let wasm = std::fs::read(support::build_bybit_venue()).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm-paged"), Some(server.uri()))
        .expect("bybit-venue must load");

    let error = loaded
        .instruments()
        .expect_err("a catalogue with a next-page cursor must not come back as a complete one");
    assert!(
        format!("{error:?}").contains("more than one page"),
        "the refusal must say why, got: {error:?}"
    );
}
