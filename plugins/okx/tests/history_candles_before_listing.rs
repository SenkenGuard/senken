//! Pins a fact confirmed live against `GET /market/history-candles`: a
//! window entirely before an instrument ever listed answers `HTTP 200` with
//! `{"code":"0","data":[]}` — an empty array, not an error, not a
//! duplicated row, not a wraparound to unrelated candles. This nails that
//! specific behaviour down rather than relying on the general
//! `len() < max_rows()` heuristic a caller might otherwise lean on to guess
//! "no more history", which this fixture proves is unnecessary: an
//! out-of-range window is answered with a plain empty page.
//!
//! Fixture recorded live, 2026-09-01:
//! `GET https://www.okx.com/api/v5/market/history-candles?instId=BTC-USDT&bar=1m&limit=100&after=1000000000&before=999999999`
//! (a one-second window in September 2001, long before OKX or BTC-USDT
//! existed) returned `HTTP 200`, body `{"code":"0","data":[],"msg":""}`
//! verbatim — see `fixtures/history_candles_before_listing.json`.

use senken_core::{TimeRange, UnixNanos};
use senken_marketdata::{Instrument, SourceSymbol};
use senken_plugin::BarSource;
use senken_plugin_okx::bar_source;
use senken_series::{BarSpec, BarUnit};
use senken_venue::{LimitGroup, VenueClient};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const FIXTURE: &[u8] = include_bytes!("fixtures/history_candles_before_listing.json");

fn btc_usdt() -> SourceSymbol {
    Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT").source_symbol()
}

fn test_client() -> VenueClient {
    VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
}

/// A window long before any instrument could have listed — the same shape
/// of request the live capture above used.
fn window_before_any_listing() -> TimeRange {
    TimeRange::new(
        UnixNanos::from_millis(999_999_999_000).unwrap(),
        UnixNanos::from_millis(1_000_000_000_000).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn a_window_entirely_before_listing_returns_an_empty_page_not_an_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(FIXTURE, "application/json"))
        .mount(&server)
        .await;
    let source = bar_source(senken_plugin_okx::SPOT_ID, test_client()).with_url(server.uri());

    let bars = source
        .bars(
            &btc_usdt(),
            BarSpec::new(1, BarUnit::Minute),
            window_before_any_listing(),
        )
        .await
        .unwrap();

    assert_eq!(
        bars,
        Vec::new(),
        "an empty `data` array in a 200 response must decode to Ok(vec![]), never an error"
    );
}
