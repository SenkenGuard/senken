//! Proves `wit/senken.wit`'s `venue-plugin` world end to end: a component
//! that tries a socket instead of the host's `fetch` fails to *load*, a
//! well-behaved one returns bars matching a genuine recorded response, and
//! the `senken_venue::LimitGroup` budget it was loaded with actually holds
//! rather than merely existing.
//!
//! See `tests/support/mod.rs` for how each fixture is compiled, and
//! `tests/fixtures/venue-*/src/lib.rs` for what each one actually does.

mod support;

use std::sync::Arc;
use std::time::Duration;

use senken_plugin_host::{
    PluginHost, PluginHostError, PluginLimits, VenueInstrumentKind, VenueInstrumentStatus,
    VenueSettlement,
};
use senken_venue::{LimitGroup, VenueClient};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// OKX's own `GET /api/v5/public/instruments?instType=SPOT` response,
/// captured live — the same fixture `plugins/okx`'s own tests decode.
const INSTRUMENTS: &[u8] = include_bytes!("../../../plugins/okx/tests/fixtures/instruments.json");
/// OKX's own `GET /api/v5/market/history-candles` response for `BTC-USDT`
/// at `1m`, captured live — the same fixture `plugins/okx`'s own
/// `OkxBarSource` tests decode, and whose expected values (four confirmed
/// bars, the newest unconfirmed row dropped, `first.open == 780_343` at
/// scale 1) this test asserts again here, against a component that reached
/// the bytes only through this crate's `fetch` bridge.
const CANDLES: &[u8] = include_bytes!("../../../plugins/okx/tests/fixtures/candles_1m.json");

fn test_client(group: LimitGroup) -> VenueClient {
    VenueClient::new(reqwest::Client::new(), group)
}

#[test]
fn a_venue_plugin_that_tries_a_socket_fails_to_load() {
    let wasm = std::fs::read(support::build_fixture("venue-tries-socket")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let client = test_client(LimitGroup::new("tries-socket"));

    let err = host
        .load_venue(&wasm, client, Some("http://example.invalid".to_owned()))
        .expect_err("a component that can reach for a socket must never load");

    assert!(
        matches!(err, PluginHostError::Load(_)),
        "expected a plain load failure (no `wasi:sockets` is ever linked), got {err:?}"
    );
}

/// Loads the fixture component against a mock OKX, the one setup every test
/// below shares. Split per market kind rather than asserted in one function:
/// each kind is its own property, and a failure should name which kind broke
/// instead of stopping at the first one.
async fn load_against_mock_okx(server: &MockServer) -> senken_plugin_host::LoadedVenuePlugin {
    Mock::given(method("GET"))
        .and(path("/api/v5/public/instruments"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(INSTRUMENTS, "application/json"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v5/market/history-candles"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
        .mount(server)
        .await;

    let wasm = std::fs::read(support::build_fixture("venue-example")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let client = test_client(LimitGroup::new("example-okx"));
    host.load_venue(&wasm, client, Some(server.uri()))
        .expect("a well-behaved venue component must load")
}

#[tokio::test]
async fn a_well_behaved_venue_plugin_loads_under_its_own_descriptor_id() {
    let server = MockServer::start().await;
    let loaded = load_against_mock_okx(&server).await;
    assert_eq!(loaded.descriptor().id, "example-okx");
}

#[tokio::test]
async fn a_spot_instrument_crosses_with_its_scales_and_carries_no_contract() {
    let server = MockServer::start().await;
    let loaded = load_against_mock_okx(&server).await;
    let instruments = loaded
        .instruments()
        .expect("the mocked instrument catalog must decode");
    let btc = instruments
        .iter()
        .find(|i| i.symbol == "BTCUSDT")
        .expect("BTC-USDT must survive this fixture's own minimal parser");
    assert_eq!(btc.source_symbol, "BTC-USDT");
    assert_eq!((btc.price_scale, btc.tick_size), (1, 1));
    assert_eq!((btc.qty_scale, btc.step_size), (8, 1));
    assert!(
        instruments.iter().all(|i| i.symbol != "OLDUSDT"),
        "a suspended instrument must not be listed as tradable"
    );
    assert_eq!(btc.kind, VenueInstrumentKind::Spot);
    assert_eq!(btc.status, VenueInstrumentStatus::Trading);
    assert!(btc.contract.is_none(), "spot must carry no contract");
}

#[tokio::test]
async fn a_perpetual_crosses_with_no_expiry_rather_than_a_sentinel_date() {
    let server = MockServer::start().await;
    let loaded = load_against_mock_okx(&server).await;
    let instruments = loaded
        .instruments()
        .expect("the mocked instrument catalog must decode");
    // Every non-spot market kind `wit/senken.wit`'s `instrument` record now
    // names must cross the boundary intact, not only spot — this is the
    // whole reason that record grew a `kind`/`status`/`contract` in the
    // first place.
    let perpetual = instruments
        .iter()
        .find(|i| i.symbol == "BTCUSD")
        .expect("the fixture's perpetual must be present");
    assert_eq!(perpetual.kind, VenueInstrumentKind::Perpetual);
    let perpetual_contract = perpetual
        .contract
        .as_ref()
        .expect("a perpetual must carry a contract");
    assert_eq!(perpetual_contract.settlement, VenueSettlement::Inverse);
    assert_eq!(perpetual_contract.settle, "BTC");
    assert_eq!(
        perpetual_contract.expiry, None,
        "a perpetual never expires — no expiry, not a far-future sentinel"
    );
    assert_eq!(
        (
            perpetual_contract.size_scale,
            perpetual_contract.contract_size
        ),
        (0, 100)
    );
    assert!(perpetual_contract.option.is_none());
}

#[tokio::test]
async fn a_dated_future_carries_its_expiry_as_instant_nanoseconds() {
    let server = MockServer::start().await;
    let loaded = load_against_mock_okx(&server).await;
    let instruments = loaded
        .instruments()
        .expect("the mocked instrument catalog must decode");
    let future = instruments
        .iter()
        .find(|i| i.symbol == "BTCUSD260904")
        .expect("the fixture's dated future must be present");
    assert_eq!(future.kind, VenueInstrumentKind::Future);
    let future_contract = future
        .contract
        .as_ref()
        .expect("a future must carry a contract");
    assert_eq!(
        future_contract.expiry,
        Some(1_788_508_800_000_000_000),
        "a dated future must carry its expiry as instant nanoseconds"
    );
}

#[tokio::test]
async fn an_option_carries_its_strike_right_and_expiry() {
    let server = MockServer::start().await;
    let loaded = load_against_mock_okx(&server).await;
    let instruments = loaded
        .instruments()
        .expect("the mocked instrument catalog must decode");
    let option = instruments
        .iter()
        .find(|i| i.symbol == "BTCUSD260830C70000")
        .expect("the fixture's option must be present");
    assert_eq!(option.kind, VenueInstrumentKind::Option);
    let option_contract = option
        .contract
        .as_ref()
        .expect("an option must carry a contract");
    assert_eq!(
        option_contract.expiry,
        Some(1_788_076_800_000_000_000),
        "an option's own expiry must cross intact alongside its strike"
    );
    let terms = option_contract
        .option
        .as_ref()
        .expect("an option instrument must carry its strike and right");
    assert_eq!(terms.right, senken_plugin_host::VenueOptionRight::Call);
    assert_eq!((terms.strike_scale, terms.strike), (0, 70_000));
}

#[tokio::test]
async fn bars_decode_ascending_with_the_unconfirmed_newest_row_dropped() {
    let server = MockServer::start().await;
    let loaded = load_against_mock_okx(&server).await;
    let bars = loaded
        .bars(
            "BTC-USDT",
            senken_plugin_host::BarSpec {
                step: 1,
                unit: senken_plugin_host::BarUnit::Minute,
            },
            0,
            i64::MAX,
        )
        .expect("the mocked candle page must decode");

    assert_eq!(bars.len(), 4, "the unconfirmed newest row must be dropped");
    assert!(
        bars.windows(2).all(|w| w[0].ts_open < w[1].ts_open),
        "bars must be ascending despite OKX returning them descending"
    );
    let first = &bars[0];
    assert_eq!(first.ts_open, 1_788_083_040_000 * 1_000_000);
    assert_eq!(first.open.scale, 1);
    assert_eq!(first.open.value, 780_343);
    assert_eq!(first.high.value, 780_401);
    assert_eq!(first.low.value, 780_342);
    assert_eq!(first.close.value, 780_401);
}

#[tokio::test]
async fn a_venue_plugins_limit_group_budget_actually_holds() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v5/public/instruments"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(INSTRUMENTS, "application/json")
                // Slow enough that the second call below is still waiting
                // on the sole concurrency permit well past this test's own
                // short timeout, without making the test itself slow to
                // run when the budget is (correctly) serializing the two.
                .set_delay(Duration::from_millis(300)),
        )
        .mount(&server)
        .await;

    let wasm = std::fs::read(support::build_fixture("venue-example")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    // A concurrency ceiling of exactly one: the second call must wait for
    // the first to finish and release its permit, never run alongside it.
    let group = LimitGroup::new("budget-test").max_concurrent(1);
    let client = test_client(group);
    let loaded = Arc::new(
        host.load_venue(&wasm, client, Some(server.uri()))
            .expect("a well-behaved venue component must load"),
    );

    let first = {
        let loaded = Arc::clone(&loaded);
        tokio::task::spawn_blocking(move || loaded.instruments())
    };
    // Give the first call time to actually acquire the sole permit before
    // the second one is even attempted, so this proves queuing rather than
    // a race between the two starting.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let second = {
        let loaded = Arc::clone(&loaded);
        tokio::task::spawn_blocking(move || loaded.instruments())
    };

    assert!(
        tokio::time::timeout(Duration::from_millis(100), second)
            .await
            .is_err(),
        "a second call must be held behind the first while the group's \
         concurrency permit is exhausted, not let through in parallel"
    );

    first
        .await
        .expect("the blocking task itself must not panic")
        .expect("the first call must still succeed once it runs");
}
