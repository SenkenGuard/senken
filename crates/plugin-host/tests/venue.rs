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

use senken_plugin_host::{PluginHost, PluginHostError, PluginLimits};
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

#[tokio::test]
async fn a_well_behaved_venue_plugin_returns_instruments_and_bars_matching_the_recorded_fixture() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v5/public/instruments"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(INSTRUMENTS, "application/json"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/api/v5/market/history-candles"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(CANDLES, "application/json"))
        .mount(&server)
        .await;

    let wasm = std::fs::read(support::build_fixture("venue-example")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let client = test_client(LimitGroup::new("example-okx"));
    let loaded = host
        .load_venue(&wasm, client, Some(server.uri()))
        .expect("a well-behaved venue component must load");

    assert_eq!(loaded.descriptor().id, "example-okx");

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
