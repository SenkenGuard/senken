//! Proves `wasm/`'s `okx-venue` component — the one that will actually run
//! in production once OKX is enabled — returns **exactly** what the native
//! adapter already returns for the same recorded bytes.
//!
//! This is the test the whole porting wave exists to have: every one of
//! the twenty-one venues left after OKX and Bybit copies this pattern, so
//! a wrong constant, a diverging normalisation rule, or a rescaled price
//! is caught here rather than trusted on the strength of "it compiled".
//!
//! Both sides answer from the very same `wiremock` server, serving
//! `plugins/okx/tests/fixtures/instruments.json` and `candles_1m.json`
//! verbatim — the exact fixtures `crates/plugin_host`'s own generic
//! `venue-example` test and `plugins/okx/src/{lib,bars}.rs`'s own unit
//! tests already decode, so this test adds no new recorded data of its
//! own, only a third caller comparing the first two.

mod support;

use okx_core::normalise_okx_symbol;
use senken_core::{TimeRange, UnixNanos};
use senken_marketdata::source::MarketDataSource;
use senken_marketdata::{Instrument, SourceSymbol};
use senken_plugin::BarSource;
use senken_plugin_host::{PluginHost, PluginLimits};
use senken_venue::{LimitGroup, VenueClient};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const INSTRUMENTS: &[u8] = include_bytes!("fixtures/instruments.json");
const CANDLES: &[u8] = include_bytes!("fixtures/candles_1m.json");
const SWAP: &[u8] = include_bytes!("fixtures/swap.json");
const FUTURES: &[u8] = include_bytes!("fixtures/futures.json");
const OPTION: &[u8] = include_bytes!("fixtures/option.json");

fn test_client(name: &str) -> VenueClient {
    VenueClient::new(reqwest::Client::new(), LimitGroup::new(name))
}

fn btc_usdt() -> SourceSymbol {
    Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT").source_symbol()
}

fn wide_range() -> TimeRange {
    TimeRange::new(
        UnixNanos::EPOCH,
        UnixNanos::from_millis(4_102_444_800_000).unwrap(),
    )
    .unwrap()
}

async fn mock_okx_server() -> MockServer {
    mock_instruments_server(INSTRUMENTS).await
}

/// A mock server serving `instruments_body` for the catalog endpoint and
/// [`CANDLES`] for history candles, regardless of query string — every
/// market's `instId`/`instType`/`instFamily` differs only in the query, and
/// `wiremock`'s `path` matcher does not look at it, so one server shape
/// serves every market's parity test.
async fn mock_instruments_server(instruments_body: &'static [u8]) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v5/public/instruments"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(instruments_body, "application/json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v5/market/history-candles"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
        .mount(&server)
        .await;
    server
}

#[tokio::test]
async fn wasm_instruments_are_identical_to_native_for_the_recorded_fixture() {
    let server = mock_okx_server().await;

    let native = senken_plugin_okx::spot_source(test_client("native"))
        .with_url(format!("{}/api/v5/public/instruments", server.uri()))
        .instruments()
        .await
        .expect("the native spot source must decode the recorded fixture");

    let wasm = std::fs::read(support::build_okx_venue()).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm"), Some(server.uri()))
        .expect("okx-venue must load");
    let wasm_instruments = loaded
        .instruments()
        .expect("okx-venue must decode the recorded fixture");

    // `wit/senken.wit`'s `instrument` record has no status field, so a
    // suspended row (`OLD-USDT`, `Halted` natively) cannot be represented
    // at all and `okx_core::parse_spot_instruments` omits it outright
    // rather than the native catalog's "listed, marked Halted" — the same
    // choice `crates/plugin-host/tests/fixtures/venue-example` already
    // made for this exact fixture. "Identical" therefore means identical
    // over the instruments the wasm side is even capable of naming: every
    // *tradable* native spot instrument, field for field.
    let tradable_native: Vec<_> = native
        .iter()
        .filter(|i| i.status == senken_marketdata::instrument::InstrumentStatus::Trading)
        .collect();
    assert!(
        native.len() > tradable_native.len(),
        "this fixture must still contain a non-tradable row (OLD-USDT), or this test no longer \
         proves the wasm side actually omits it rather than merely matching a fixture with \
         nothing to omit"
    );
    assert!(
        wasm_instruments.iter().all(|i| i.symbol != "OLDUSDT"),
        "a suspended instrument has no status to carry over the wasm boundary and must not appear"
    );
    assert!(
        !tradable_native.is_empty() && !wasm_instruments.is_empty(),
        "the fixture must actually produce instruments on both sides, or this test proves nothing"
    );
    assert_eq!(
        tradable_native.len(),
        wasm_instruments.len(),
        "native (tradable only): {tradable_native:#?}\nwasm: {wasm_instruments:#?}"
    );

    for expected in &tradable_native {
        let actual = wasm_instruments
            .iter()
            .find(|i| i.symbol == expected.symbol)
            .unwrap_or_else(|| {
                panic!(
                    "{} present natively but missing from okx-venue",
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
    let server = mock_okx_server().await;

    let native = senken_plugin_okx::bar_source(senken_plugin_okx::SPOT_ID, test_client("native"))
        .with_url(format!("{}/api/v5/market/history-candles", server.uri()))
        .bars(
            &btc_usdt(),
            senken_series::BarSpec::new(1, senken_series::BarUnit::Minute),
            wide_range(),
        )
        .await
        .expect("the native bar source must decode the recorded fixture");

    let wasm = std::fs::read(support::build_okx_venue()).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm"), Some(server.uri()))
        .expect("okx-venue must load");
    let wasm_bars = loaded
        .bars(
            "BTC-USDT",
            senken_plugin_host::BarSpec {
                step: 1,
                unit: senken_plugin_host::BarUnit::Minute,
            },
            0,
            i64::MAX,
        )
        .expect("okx-venue must decode the recorded fixture");

    assert!(
        !native.is_empty() && !wasm_bars.is_empty(),
        "the fixture must actually produce bars on both sides, or this test proves nothing"
    );
    assert_eq!(native.len(), wasm_bars.len());

    for (expected, actual) in native.iter().zip(wasm_bars.iter()) {
        // `senken_series::Bar`'s own price/quantity fields carry no scale
        // of their own (a real series' scale lives once on the series, not
        // once per bar) — only the wasm side's WIT `bar` does, per that
        // record's own doc comment. `1` is this exact fixture's own known
        // price scale (`plugins/okx/src/bars.rs`'s own
        // `fixture_rows_decode_…` test asserts the same number for the
        // same bytes), so an equality here on top of the raw values below
        // still catches a rescale that happened to keep every raw integer
        // the same but changed what it means.
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
            panic!("OKX always reports real volume");
        };
        let senken_series::Volume::Real(expected_volume) = expected.volume else {
            panic!("OKX always reports real volume");
        };
        assert_eq!(actual_volume.value, expected_volume);
        assert_eq!(actual.quote_volume.map(|q| q.value), expected.quote_volume);
    }
}

/// The property `AGENTS.md` names directly: a venue's symbol is
/// normalised **once**, by one rule. The wasm component's own instrument
/// catalog (loaded through the real `PluginHost`, from the real recorded
/// fixture) is checked here against `okx_core::normalise_okx_symbol`
/// called directly on each row's `instId` — the same function
/// `plugins/okx/src/lib.rs`'s native swap/futures/option catalog builder
/// and `plugins/okx/src/feed.rs`'s live decoder both call (see those
/// modules' own
/// `swaps_take_their_pair_from_uly_since_base_ccy_is_empty`/
/// `the_live_decoder_normalises_a_perpetual_swaps_instid_the_same_way_the_catalog_does`
/// unit tests for the other two call sites this integration test cannot
/// reach — both are crate-private). If any call site ever computed its
/// own separator rule instead of calling this function, this assertion
/// would not itself catch it (each call site is checked against the same
/// function it calls), but together with those two unit tests it pins all
/// three call sites to identical output for every market shape this
/// fixture carries.
#[tokio::test]
async fn the_wasm_catalogs_symbols_match_normalise_okx_symbol_for_every_row() {
    let server = mock_okx_server().await;
    let wasm = std::fs::read(support::build_okx_venue()).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm-symbols"), Some(server.uri()))
        .expect("okx-venue must load");
    let instruments = loaded
        .instruments()
        .expect("okx-venue must decode the recorded fixture");

    assert!(!instruments.is_empty());
    for instrument in &instruments {
        assert_eq!(
            instrument.symbol,
            normalise_okx_symbol(&instrument.source_symbol),
            "{} did not normalise the way okx_core::normalise_okx_symbol does",
            instrument.source_symbol
        );
    }
}

/// `true` when a domain `Settlement` and a WIT `VenueSettlement` name the
/// same case — the two are different types (one crosses the plugin
/// boundary, one does not), so this is a case-for-case match rather than
/// a derived `PartialEq`.
fn settlements_match(
    domain: senken_marketdata::instrument::Settlement,
    wit: senken_plugin_host::VenueSettlement,
) -> bool {
    use senken_marketdata::instrument::Settlement as D;
    use senken_plugin_host::VenueSettlement as W;
    matches!(
        (domain, wit),
        (D::Linear, W::Linear) | (D::Inverse, W::Inverse) | (D::Quanto, W::Quanto)
    )
}

/// `true` when a domain `OptionRight` and a WIT `VenueOptionRight` name the
/// same case — the same reasoning as [`settlements_match`].
fn option_rights_match(
    domain: senken_marketdata::instrument::OptionRight,
    wit: senken_plugin_host::VenueOptionRight,
) -> bool {
    use senken_marketdata::instrument::OptionRight as D;
    use senken_plugin_host::VenueOptionRight as W;
    matches!((domain, wit), (D::Call, W::Call) | (D::Put, W::Put))
}

/// Asserts every field of one derivative instrument's contract matches
/// between the native (`expected`) and wasm (`actual`) sides — the
/// contract-crossing property `crates/plugin-host/tests/venue.rs` already
/// proves generically; this proves OKX's own real recorded swap/futures
/// data crosses identically through both real pipelines.
fn assert_contracts_match(
    expected: &senken_marketdata::instrument::Contract,
    actual: &senken_plugin_host::VenueContract,
) {
    assert_eq!(actual.settle, expected.settle);
    assert!(
        settlements_match(expected.settlement, actual.settlement),
        "settlement mismatch: native {:?} vs wasm {:?}",
        expected.settlement,
        actual.settlement
    );
    assert_eq!(
        actual.expiry,
        expected.expiry.map(UnixNanos::as_nanos),
        "expiry must cross as instant nanoseconds identically, including a perpetual's `None`"
    );
    assert_eq!(
        (actual.size_scale, actual.contract_size),
        (expected.size_scale, expected.contract_size)
    );
}

#[tokio::test]
async fn wasm_swap_instruments_are_identical_to_native_for_the_recorded_fixture() {
    let server = mock_instruments_server(SWAP).await;

    let native = senken_plugin_okx::swap_source(test_client("native"))
        .with_url(format!("{}/api/v5/public/instruments", server.uri()))
        .instruments()
        .await
        .expect("the native swap source must decode the recorded fixture");

    let wasm = std::fs::read(support::build_okx_venue_component(
        "wasm-swap",
        "okx_venue_swap",
    ))
    .unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm-swap"), Some(server.uri()))
        .expect("okx-venue-swap must load");
    assert_eq!(
        loaded.descriptor().id,
        "okx-swap",
        "the source id half of every saved chart layout's instrument id must not change"
    );
    let wasm_instruments = loaded
        .instruments()
        .expect("okx-venue-swap must decode the recorded fixture");

    assert!(
        !native.is_empty() && !wasm_instruments.is_empty(),
        "the fixture must actually produce instruments on both sides, or this test proves nothing"
    );
    assert_eq!(native.len(), wasm_instruments.len());

    for expected in &native {
        let actual = wasm_instruments
            .iter()
            .find(|i| i.symbol == expected.symbol)
            .unwrap_or_else(|| {
                panic!(
                    "{} present natively but missing from okx-venue-swap",
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

        let expected_contract = expected
            .contract
            .as_ref()
            .expect("a swap must carry a contract natively");
        let actual_contract = actual
            .contract
            .as_ref()
            .expect("a swap must carry a contract over the wasm boundary too");
        assert_contracts_match(expected_contract, actual_contract);
    }
}

#[tokio::test]
async fn wasm_swap_bars_are_identical_to_native_for_the_recorded_fixture() {
    let server = mock_instruments_server(SWAP).await;

    let native = senken_plugin_okx::bar_source(senken_plugin_okx::SWAP_ID, test_client("native"))
        .with_url(format!("{}/api/v5/market/history-candles", server.uri()))
        .bars(
            &btc_usdt(),
            senken_series::BarSpec::new(1, senken_series::BarUnit::Minute),
            wide_range(),
        )
        .await
        .expect("the native swap bar source must decode the recorded fixture");

    let wasm = std::fs::read(support::build_okx_venue_component(
        "wasm-swap",
        "okx_venue_swap",
    ))
    .unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm-swap-bars"), Some(server.uri()))
        .expect("okx-venue-swap must load");
    let wasm_bars = loaded
        .bars(
            "BTC-USDT",
            senken_plugin_host::BarSpec {
                step: 1,
                unit: senken_plugin_host::BarUnit::Minute,
            },
            0,
            i64::MAX,
        )
        .expect("okx-venue-swap must decode the recorded fixture");

    assert!(!native.is_empty() && !wasm_bars.is_empty());
    assert_eq!(native.len(), wasm_bars.len());
    for (expected, actual) in native.iter().zip(wasm_bars.iter()) {
        assert_eq!(actual.ts_open, expected.ts_open.as_nanos());
        assert_eq!(actual.open.value, expected.open);
        assert_eq!(actual.close.value, expected.close);
    }
}

#[tokio::test]
async fn wasm_futures_instruments_are_identical_to_native_for_the_recorded_fixture() {
    let server = mock_instruments_server(FUTURES).await;

    let native = senken_plugin_okx::futures_source(test_client("native"))
        .with_url(format!("{}/api/v5/public/instruments", server.uri()))
        .instruments()
        .await
        .expect("the native futures source must decode the recorded fixture");

    let wasm = std::fs::read(support::build_okx_venue_component(
        "wasm-futures",
        "okx_venue_futures",
    ))
    .unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm-futures"), Some(server.uri()))
        .expect("okx-venue-futures must load");
    assert_eq!(
        loaded.descriptor().id,
        "okx-futures",
        "the source id half of every saved chart layout's instrument id must not change"
    );
    let wasm_instruments = loaded
        .instruments()
        .expect("okx-venue-futures must decode the recorded fixture");

    assert!(
        !native.is_empty() && !wasm_instruments.is_empty(),
        "the fixture must actually produce instruments on both sides, or this test proves nothing"
    );
    assert_eq!(native.len(), wasm_instruments.len());

    for expected in &native {
        let actual = wasm_instruments
            .iter()
            .find(|i| i.symbol == expected.symbol)
            .unwrap_or_else(|| {
                panic!(
                    "{} present natively but missing from okx-venue-futures",
                    expected.symbol
                )
            });
        assert_eq!(actual.source_symbol, expected.source_symbol);
        assert_eq!(actual.base, expected.base);
        assert_eq!(actual.quote, expected.quote);
        assert_eq!(actual.price_scale, expected.price_scale);
        assert_eq!(actual.tick_size, expected.tick_size);

        let expected_contract = expected
            .contract
            .as_ref()
            .expect("a dated future must carry a contract natively");
        let actual_contract = actual
            .contract
            .as_ref()
            .expect("a dated future must carry a contract over the wasm boundary too");
        assert_contracts_match(expected_contract, actual_contract);
        assert!(
            actual_contract.expiry.is_some(),
            "a dated future's expiry must survive the crossing, not just its presence natively"
        );
    }
}

#[tokio::test]
async fn wasm_futures_bars_are_identical_to_native_for_the_recorded_fixture() {
    let server = mock_instruments_server(FUTURES).await;

    let native =
        senken_plugin_okx::bar_source(senken_plugin_okx::FUTURES_ID, test_client("native"))
            .with_url(format!("{}/api/v5/market/history-candles", server.uri()))
            .bars(
                &btc_usdt(),
                senken_series::BarSpec::new(1, senken_series::BarUnit::Minute),
                wide_range(),
            )
            .await
            .expect("the native futures bar source must decode the recorded fixture");

    let wasm = std::fs::read(support::build_okx_venue_component(
        "wasm-futures",
        "okx_venue_futures",
    ))
    .unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client("wasm-futures-bars"), Some(server.uri()))
        .expect("okx-venue-futures must load");
    let wasm_bars = loaded
        .bars(
            "BTC-USDT",
            senken_plugin_host::BarSpec {
                step: 1,
                unit: senken_plugin_host::BarUnit::Minute,
            },
            0,
            i64::MAX,
        )
        .expect("okx-venue-futures must decode the recorded fixture");

    assert!(!native.is_empty() && !wasm_bars.is_empty());
    assert_eq!(native.len(), wasm_bars.len());
    for (expected, actual) in native.iter().zip(wasm_bars.iter()) {
        assert_eq!(actual.ts_open, expected.ts_open.as_nanos());
        assert_eq!(actual.open.value, expected.open);
        assert_eq!(actual.close.value, expected.close);
    }
}

/// One shared proof for both liquid option families: `wasm_dir`/
/// `binary_stem`/`source_id`/`family` are the only things that differ
/// between `okx-venue-option-btc-usd` and `okx-venue-option-eth-usd`.
async fn assert_option_family_parity(
    wasm_dir: &str,
    binary_stem: &str,
    family: &str,
    expected_source_id: &str,
) {
    let server = mock_instruments_server(OPTION).await;

    let native = senken_plugin_okx::option_source(test_client("native"), family)
        .with_url(format!("{}/api/v5/public/instruments", server.uri()))
        .instruments()
        .await
        .unwrap_or_else(|error| panic!("the native {family} option source must decode: {error}"));
    assert_eq!(
        senken_plugin_okx::option_source(test_client("id-check"), family).id(),
        expected_source_id,
        "the native source's own id must keep matching the component's"
    );

    let wasm = std::fs::read(support::build_okx_venue_component(wasm_dir, binary_stem)).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_venue(&wasm, test_client(binary_stem), Some(server.uri()))
        .unwrap_or_else(|error| panic!("{binary_stem} must load: {error}"));
    assert_eq!(
        loaded.descriptor().id,
        expected_source_id,
        "the source id half of every saved chart layout's instrument id must not change"
    );
    let wasm_instruments = loaded
        .instruments()
        .unwrap_or_else(|error| panic!("{binary_stem} must decode the recorded fixture: {error}"));

    assert!(
        !native.is_empty() && !wasm_instruments.is_empty(),
        "the fixture must actually produce instruments on both sides, or this test proves nothing"
    );
    assert_eq!(native.len(), wasm_instruments.len());

    for expected in &native {
        let actual = wasm_instruments
            .iter()
            .find(|i| i.symbol == expected.symbol)
            .unwrap_or_else(|| {
                panic!(
                    "{} present natively but missing from {binary_stem}",
                    expected.symbol
                )
            });
        assert_eq!(actual.source_symbol, expected.source_symbol);
        assert_eq!(actual.base, expected.base);
        assert_eq!(actual.quote, expected.quote);

        let expected_contract = expected
            .contract
            .as_ref()
            .expect("an option must carry a contract natively");
        let actual_contract = actual
            .contract
            .as_ref()
            .expect("an option must carry a contract over the wasm boundary too");
        assert_contracts_match(expected_contract, actual_contract);

        let expected_terms = expected_contract
            .option
            .as_ref()
            .expect("an option's contract must carry its strike and right natively");
        let actual_terms = actual_contract
            .option
            .as_ref()
            .expect("an option's strike and right must survive the crossing too");
        assert!(
            option_rights_match(expected_terms.right, actual_terms.right),
            "right (call/put) must match — native {:?}, wasm {:?}",
            expected_terms.right,
            actual_terms.right
        );
        assert_eq!(
            (actual_terms.strike_scale, actual_terms.strike),
            (expected_terms.strike_scale, expected_terms.strike)
        );
    }
}

#[tokio::test]
async fn wasm_btc_usd_option_instruments_are_identical_to_native_for_the_recorded_fixture() {
    assert_option_family_parity(
        "wasm-option-btc-usd",
        "okx_venue_option_btc_usd",
        "BTC-USD",
        "okx-option-btc-usd",
    )
    .await;
}

#[tokio::test]
async fn wasm_eth_usd_option_instruments_are_identical_to_native_for_the_recorded_fixture() {
    assert_option_family_parity(
        "wasm-option-eth-usd",
        "okx_venue_option_eth_usd",
        "ETH-USD",
        "okx-option-eth-usd",
    )
    .await;
}
