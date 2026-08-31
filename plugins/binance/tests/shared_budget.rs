//! A `MarketDataSource` and a `BarSource` for this venue must share one
//! rate/concurrency budget, not one each —
//! `crates/plugin`'s own `limit_group` tests prove the mechanism generically;
//! this proves it end to end through this crate's real registration path,
//! the same way `BinancePlugin::activate` builds both.

use std::sync::Arc;
use std::time::Duration;

use senken_marketdata::{Instrument, MarketDataSource};
use senken_plugin::{ActivationContext, BarSource};
use senken_plugin_binance::{bar_source_spot, spot_source};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

struct ManualClock;

#[async_trait::async_trait]
impl senken_series::Clock for ManualClock {
    fn now(&self) -> senken_core::UnixNanos {
        senken_core::UnixNanos::EPOCH
    }

    async fn sleep_until(&self, _t: senken_core::UnixNanos) {}
}

#[tokio::test]
async fn a_bar_source_and_a_marketdata_source_for_binance_share_one_budget() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&server)
        .await;

    let mut context = ActivationContext::new();
    // Exactly `BinancePlugin::activate`'s own shape: one named group, two
    // independent `venue_client` calls, budgeted to two requests total.
    let group = context
        .limit_group("binance")
        .per_window(Duration::from_mins(1), 2);
    let instrument_client = context.venue_client(&group).unwrap();
    let bar_client = context.venue_client(&group).unwrap();

    let instruments = spot_source(instrument_client).with_url(server.uri());
    let bars = bar_source_spot(bar_client, Arc::new(ManualClock)).with_url(server.uri());

    // Spend the shared budget of two: one instrument fetch, one bar fetch.
    instruments.instruments().await.unwrap();
    let range = senken_core::TimeRange::new(
        senken_core::UnixNanos::EPOCH,
        senken_core::UnixNanos::from_millis(60_000).unwrap(),
    )
    .unwrap();
    // The mock body `[]` is a valid (empty) klines array — this call still
    // spends the shared group's proactive budget regardless of how many
    // rows come back. `SourceSymbol` is only obtainable
    // from an `Instrument`, never a bare string literal.
    let symbol = Instrument::spot("BTCUSDT", "BTCUSDT", "BTC", "USDT").source_symbol();
    bars.bars(
        &symbol,
        senken_series::BarSpec::new(1, senken_series::BarUnit::Minute),
        range,
    )
    .await
    .unwrap();

    // A third request of either kind must now wait: with F1 unfixed, each
    // `limit_group("binance")` call would have returned an independent,
    // untouched budget and this would succeed immediately instead.
    let waited = tokio::time::timeout(Duration::from_millis(50), instruments.instruments()).await;
    assert!(
        waited.is_err(),
        "instrument and bar traffic for one venue must share one budget"
    );
}
