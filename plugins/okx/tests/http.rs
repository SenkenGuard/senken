//! The HTTP path against a local mock venue: status handling, application
//! errors inside a 200 body, transport failures, and the `with_url`
//! override.

use senken_marketdata::source::{MarketDataSource, SourceError};
use senken_plugin_okx::spot_source;
use senken_venue::{LimitGroup, VenueClient};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

const FIXTURE: &[u8] = include_bytes!("fixtures/instruments.json");

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
    assert_eq!(instruments.len(), 4);
}

#[tokio::test]
async fn an_application_error_in_a_200_body_is_a_rejection() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"code":"50011","msg":"Rate limit reached","data":[]}"#),
        )
        .mount(&server)
        .await;

    let error = source(server.uri()).instruments().await.unwrap_err();
    assert!(matches!(error, SourceError::Rejected { .. }));
    assert!(!error.is_retryable());
}

#[tokio::test]
async fn a_server_error_is_a_retryable_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).set_body_string("maintenance"))
        .mount(&server)
        .await;

    let error = source(server.uri()).instruments().await.unwrap_err();
    assert!(matches!(error, SourceError::Http { status: 503, .. }));
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
