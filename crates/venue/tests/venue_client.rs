//! `VenueClient` against a local mock venue: rate limiting, retry, backoff
//! and the circuit breaker.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use senken_venue::{LimitGroup, RetryPolicy, VenueClient};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

fn client(group: LimitGroup) -> VenueClient {
    VenueClient::new(reqwest::Client::new(), group)
}

#[tokio::test]
async fn a_429_with_retry_after_honours_the_venues_own_wait() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("retry-after", "1")
                .set_body_string("slow down"),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let start = tokio::time::Instant::now();
    let body = client(LimitGroup::new("retry-after"))
        .get(&server.uri(), 1)
        .await
        .unwrap();
    let elapsed = start.elapsed();

    assert_eq!(body, b"ok");
    assert!(
        elapsed >= Duration::from_millis(950),
        "must wait at least the ~1s Retry-After told it to, waited {elapsed:?}"
    );
}

#[tokio::test]
async fn a_429_without_retry_after_falls_back_to_backoff_and_still_recovers() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(429).set_body_string("slow down"))
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .expect(1)
        .mount(&server)
        .await;

    let body = client(LimitGroup::new("no-retry-after"))
        .get(&server.uri(), 1)
        .await
        .unwrap();
    assert_eq!(
        body, b"ok",
        "a bare 429 must still be retried, not surfaced"
    );
}

#[tokio::test]
async fn a_418_trips_the_circuit_and_the_group_then_fails_fast() {
    let server = MockServer::start().await;
    // Exactly one request must ever reach the venue: a 418 must not be
    // retried by `get` itself, and the *next* call must not reach the venue
    // at all because the circuit is open.
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(418).set_body_string("banned"))
        .expect(1)
        .mount(&server)
        .await;

    let group = LimitGroup::new("banned-venue");
    let venue = client(group);

    let first = venue.get(&server.uri(), 1).await.unwrap_err();
    assert!(
        matches!(
            first,
            senken_marketdata::source::SourceError::Http { status: 418, .. }
        ),
        "the 418 itself must be reported, not masked"
    );

    let start = tokio::time::Instant::now();
    let second = venue.get(&server.uri(), 1).await.unwrap_err();
    assert!(
        start.elapsed() < Duration::from_millis(100),
        "a tripped circuit must fail fast rather than queue behind the ban"
    );
    assert!(
        !second.is_retryable(),
        "the fail-fast rejection must not itself invite another retry"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn max_concurrent_is_respected_under_concurrent_load() {
    struct TrackConcurrency {
        current: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
    }
    impl Respond for TrackConcurrency {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            let now = self.current.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(now, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(80));
            self.current.fetch_sub(1, Ordering::SeqCst);
            ResponseTemplate::new(200).set_body_string("ok")
        }
    }

    let server = MockServer::start().await;
    let current = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .respond_with(TrackConcurrency {
            current: Arc::clone(&current),
            peak: Arc::clone(&peak),
        })
        .mount(&server)
        .await;

    let venue = client(LimitGroup::new("bounded").max_concurrent(2));
    let mut requests = Vec::new();
    for _ in 0..6 {
        let venue = venue.clone();
        let url = server.uri();
        requests.push(tokio::spawn(
            async move { venue.get(&url, 1).await.unwrap() },
        ));
    }
    for request in requests {
        request.await.unwrap();
    }

    let observed = peak.load(Ordering::SeqCst);
    assert!(
        observed <= 2,
        "the ceiling was 2 but {observed} requests were in flight at once"
    );
}

#[tokio::test]
async fn header_reconciliation_adopts_the_venues_own_used_weight() {
    let server = MockServer::start().await;
    // The exact header and value verified live against Binance klines
    //: `x-mbx-used-weight-1m: 750`.
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("x-mbx-used-weight-1m", "750")
                .set_body_string("ok"),
        )
        .mount(&server)
        .await;

    // A generous budget our own accounting would never approach on its own.
    let group = LimitGroup::new("binance-like").per_window(Duration::from_mins(1), 800);
    let venue = client(group);
    venue.get(&server.uri(), 1).await.unwrap();

    // The venue just told us we are at 750/800 — 50 units of headroom, not
    // 799. A second request costing 60 must therefore wait rather than be
    // admitted on our own (wrong) count of 1 used.
    let waited =
        tokio::time::timeout(Duration::from_millis(50), venue.get(&server.uri(), 60)).await;
    assert!(
        waited.is_err(),
        "reconciliation must adopt the venue's used-weight, not keep our own guess"
    );
}

#[tokio::test]
async fn a_group_with_no_headers_still_limits_from_its_own_proactive_bucket() {
    // The OKX case: the venue sends nothing to reconcile against,
    // so the proactive window must be the only thing enforcing the budget —
    // and it must still work correctly on its own.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let group = LimitGroup::new("okx-like").per_window(Duration::from_mins(1), 2);
    let venue = client(group.clone());
    venue.get(&server.uri(), 1).await.unwrap();
    venue.get(&server.uri(), 1).await.unwrap();

    let waited = tokio::time::timeout(Duration::from_millis(50), venue.get(&server.uri(), 1)).await;
    assert!(
        waited.is_err(),
        "a third request over a budget of two must wait, with no headers involved at all"
    );
}

#[tokio::test]
async fn two_clients_sharing_one_group_draw_from_one_budget_not_two() {
    // Simulates `binance-spot` and `binance-usdm`: two different sources,
    // built from clones of the same `LimitGroup`, must not each get their
    // own budget (keyed by group, not by source).
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let group = LimitGroup::new("shared-venue").per_window(Duration::from_mins(1), 2);
    let spot = client(group.clone());
    let usdm = client(group.clone());

    spot.get(&server.uri(), 1).await.unwrap();
    usdm.get(&server.uri(), 1).await.unwrap();

    // The budget of two is now spent, regardless of which client spent it.
    let waited = tokio::time::timeout(Duration::from_millis(50), spot.get(&server.uri(), 1)).await;
    assert!(
        waited.is_err(),
        "a per-source limiter would still have budget left on `usdm`'s side; \
         a per-group limiter must not"
    );
}

#[tokio::test]
async fn repeated_backoffs_from_the_same_starting_state_are_not_identical() {
    // Full jitter: the same failure, retried independently
    // several times from a fresh group each time, must not always wait the
    // same amount before its retry succeeds — otherwise 50 sources sharing
    // one TTL would still thunder together.
    async fn one_retry_delay() -> Duration {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(503).set_body_string("busy"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let policy = RetryPolicy {
            max_attempts: 2,
            first_backoff: Duration::from_millis(200),
        };
        let venue = client(LimitGroup::new("jitter-observation")).with_retry_policy(policy);
        let start = tokio::time::Instant::now();
        venue.get(&server.uri(), 1).await.unwrap();
        start.elapsed()
    }

    let mut distinct = std::collections::HashSet::new();
    for _ in 0..8 {
        // Bucketed to the nearest 5ms: real jitter, not scheduler noise.
        let bucket = one_retry_delay().await.as_millis() / 5;
        distinct.insert(bucket);
    }
    assert!(
        distinct.len() > 1,
        "8 independent retries all waited the same bucketed duration: {distinct:?}"
    );
}
