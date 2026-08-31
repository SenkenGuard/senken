//! The HTTP path against a local mock venue: status handling, transport
//! failures, and the `with_url` override.

use senken_marketdata::source::{MarketDataSource, SourceError};
use senken_plugin_binance::spot_source;
use senken_venue::{LimitGroup, VenueClient};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const FIXTURE: &[u8] = include_bytes!("fixtures/exchange_info.json");

fn source(url: impl Into<String>) -> impl MarketDataSource {
    let client = VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"));
    spot_source(client).with_url(url)
}

#[tokio::test]
async fn a_success_body_is_fetched_and_normalised() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(FIXTURE, "application/json"))
        .mount(&server)
        .await;

    let instruments = source(server.uri()).instruments().await.unwrap();
    assert_eq!(instruments.len(), 3);
}

#[tokio::test]
async fn a_rate_limit_is_a_retryable_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .mount(&server)
        .await;

    let error = source(server.uri()).instruments().await.unwrap_err();
    assert!(matches!(error, SourceError::Http { status: 429, .. }));
    assert!(error.is_retryable());
}

#[tokio::test]
async fn an_unreachable_venue_is_a_transport_error() {
    // Port 1 on loopback: binding it needs privilege, so nothing is ever
    // listening and the connection is refused without a race to lose.
    let error = source("http://127.0.0.1:1/")
        .instruments()
        .await
        .unwrap_err();
    assert!(matches!(error, SourceError::Transport { .. }));
    assert!(error.is_retryable());
}
