//! `ActivationContext::limit_group` must deduplicate by name (plan post-M3
//! one shared budget).
//!
//! Mirrors `crates/venue`'s
//! `two_clients_sharing_one_group_draw_from_one_budget_not_two`, but proves
//! the dedup one layer up: there, two `VenueClient`s are built from explicit
//! clones of *one* `LimitGroup` value. Here, two separate
//! `context.limit_group("name")` calls — as a plugin's `MarketDataSource`
//! registration and its later `BarSource` registration would each
//! make independently — must still land on that same one group, not two
//! freshly budgeted ones.

use std::time::Duration;

use senken_plugin::ActivationContext;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn two_limit_group_calls_with_the_same_name_share_one_budget() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let mut context = ActivationContext::new();

    // Two independent call sites asking for the same venue by name, exactly
    // as an instrument source and a bar source for one venue would each do
    // from the same `activate` (or even from two different plugins, since
    // the runtime reuses one context across every plugin it activates).
    let instrument_traffic_group = context
        .limit_group("shared-venue")
        .per_window(Duration::from_mins(1), 2);
    let bar_traffic_group = context.limit_group("shared-venue");

    let instrument_client = context.venue_client(&instrument_traffic_group).unwrap();
    let bar_client = context.venue_client(&bar_traffic_group).unwrap();

    // Spend the budget of two, split across both "traffic kinds".
    instrument_client.get(&server.uri(), 1).await.unwrap();
    bar_client.get(&server.uri(), 1).await.unwrap();

    // With F1 unfixed, `bar_traffic_group` would be a fresh, independently
    // budgeted `LimitGroup` with its own untouched window, and this request
    // would succeed immediately instead of waiting.
    let waited = tokio::time::timeout(
        Duration::from_millis(50),
        instrument_client.get(&server.uri(), 1),
    )
    .await;
    assert!(
        waited.is_err(),
        "two `limit_group` calls with the same name must share one budget, \
         not one each"
    );
}

#[tokio::test]
async fn limit_group_calls_with_different_names_stay_independent() {
    // The dedup must be keyed by name — two genuinely different venues must
    // not accidentally share a budget just because they share a context.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let mut context = ActivationContext::new();
    let a = context
        .limit_group("venue-a")
        .per_window(Duration::from_mins(1), 1);
    let b = context
        .limit_group("venue-b")
        .per_window(Duration::from_mins(1), 1);

    let client_a = context.venue_client(&a).unwrap();
    let client_b = context.venue_client(&b).unwrap();

    // `a` alone exhausts its own budget of one.
    client_a.get(&server.uri(), 1).await.unwrap();

    // `b` must be unaffected — a shared cache keyed on the wrong thing (or
    // not keyed at all) could plausibly cross the streams here.
    let still_free =
        tokio::time::timeout(Duration::from_millis(50), client_b.get(&server.uri(), 1)).await;
    assert!(
        still_free.is_ok(),
        "a different venue name must not share `venue-a`'s budget"
    );
}
