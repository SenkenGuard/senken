//! The two behaviours a venue source has that a parser cannot: walking a
//! venue's pages, and retrying the failures worth retrying.

use senken_marketdata::instrument::Instrument;
use senken_marketdata::source::{MarketDataSource, SourceError};
use senken_venue::{HttpSource, LimitGroup, ParseInstruments, ReadCursor, VenueClient};
use wiremock::matchers::{method, query_param, query_param_is_missing};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Decodes `{"symbols":["A","B"]}` into that many spot instruments.
const PARSE: ParseInstruments = |body| {
    let page: serde_json::Value = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(page["symbols"]
        .as_array()
        .unwrap_or(&Vec::new())
        .iter()
        .filter_map(|s| s.as_str())
        .map(|symbol| Instrument::spot(symbol, symbol, symbol, "USDT"))
        .collect())
};

/// Reads `{"cursor":"…"}`, empty on the last page.
const CURSOR: ReadCursor = |body| {
    let page: serde_json::Value = serde_json::from_slice(body).ok()?;
    let cursor = page["cursor"].as_str()?.to_owned();
    (!cursor.is_empty()).then_some(cursor)
};

fn source(url: String, parse: ParseInstruments) -> HttpSource {
    let client = VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"));
    HttpSource::new("venue", "Venue", url, client, parse)
}

#[tokio::test]
async fn a_paged_catalog_is_walked_to_the_end() {
    let server = MockServer::start().await;
    // Page one hands out a cursor; page two closes the walk.
    Mock::given(method("GET"))
        .and(query_param_is_missing("cursor"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"symbols":["AAA","BBB"],"cursor":"next one"}"#),
        )
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(query_param("cursor", "next one"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"symbols":["CCC"],"cursor":""}"#),
        )
        .mount(&server)
        .await;

    let instruments = source(server.uri(), PARSE)
        .paginated(CURSOR, "cursor", 10)
        .instruments()
        .await
        .unwrap();

    let symbols: Vec<&str> = instruments.iter().map(|i| i.symbol.as_str()).collect();
    assert_eq!(symbols, ["AAA", "BBB", "CCC"], "every page must contribute");
}

#[tokio::test]
async fn a_source_that_does_not_page_reads_one_document() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"symbols":["AAA"],"cursor":"ignored without pagination"}"#),
        )
        .mount(&server)
        .await;

    let instruments = source(server.uri(), PARSE).instruments().await.unwrap();
    assert_eq!(instruments.len(), 1);
}

#[tokio::test]
async fn the_page_limit_stops_a_venue_that_never_runs_out() {
    let server = MockServer::start().await;
    // Every page points at another one, forever.
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string(r#"{"symbols":["AAA"],"cursor":"more"}"#),
        )
        .mount(&server)
        .await;

    let instruments = source(server.uri(), PARSE)
        .paginated(CURSOR, "cursor", 3)
        .instruments()
        .await
        .unwrap();

    assert_eq!(instruments.len(), 3, "the walk stops at the page limit");
}

#[tokio::test]
async fn a_retryable_failure_is_tried_again() {
    let server = MockServer::start().await;
    // One 503, then a good page: the source must survive the first.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(503).set_body_string("maintenance"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"symbols":["AAA"]}"#))
        .mount(&server)
        .await;

    let instruments = source(server.uri(), PARSE).instruments().await.unwrap();
    assert_eq!(instruments.len(), 1, "the retry recovered the catalog");
}

#[tokio::test]
async fn a_rejection_is_never_retried() {
    let server = MockServer::start().await;
    // A 400 says the request itself is wrong; repeating it only burns
    // quota. Exactly one request must reach the venue.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(400).set_body_string("bad request"))
        .expect(1)
        .mount(&server)
        .await;

    let error = source(server.uri(), PARSE).instruments().await.unwrap_err();
    assert!(matches!(error, SourceError::Http { status: 400, .. }));
    assert!(!error.is_retryable());
}
