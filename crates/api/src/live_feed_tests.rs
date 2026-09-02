//! End-to-end proof of the wiring, against a fake venue's
//! WebSocket (never a real one — the access-boundary
//! constraint) so this suite carries no network dependency.
//!
//! Both milestones share one setup (a real `WsVenueConnector`/
//! `SubscriptionPool` dialling a loopback fake server) because the property
//! S6 cares about most — an alert outlives the chart that created it — is
//! only real proof once it is shown through the same pool a chart pane's WS
//! subscription actually leases from, not only through two directly-held
//! `Lease`s in a unit test (`senken_alerts::runner`'s own test already
//! covers that half in isolation).

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use senken_core::UnixNanos;
use senken_feed::LiveUpdate;
use senken_identity::DEFAULT_ADMIN_EMAIL;
use senken_marketdata::{InstrumentId, SourceError, SourceSymbol};
use senken_subscription::{
    BookLevel, BookSessionRegistry, BookSnapshot, BookSource, ConnectionError, PriceUpdate,
    QuoteUpdate, SubscriptionPool,
};
use senken_venue::LimitGroup;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::watch;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::test_support::{
    ADMIN_TEST_PASSWORD, body_json, get_auth, post_json, post_json_auth,
    serve_unfenced_test_server_with_book, serve_unfenced_test_server_with_feed, temp_empty_runtime,
};

const TEST_SOURCE: &str = "test-venue";

/// How long any single step of these tests may wait before it is a failure.
///
/// Every await here is on a loopback socket or an in-process task, so a
/// second is already generous and ten is beyond argument. The number matters
/// far less than the fact that there is one: an unbounded `await` turns any
/// scheduling difference into a hang rather than a failure, and `cargo test`
/// has no per-test timeout of its own. That is not hypothetical — this suite
/// hung for over half an hour on a CI runner while passing everywhere else,
/// and reported nothing about where it stopped.
const STEP_TIMEOUT: Duration = Duration::from_secs(10);

/// Awaits `future`, failing with `what` rather than hanging.
async fn within<F: Future>(what: &str, future: F) -> F::Output {
    match tokio::time::timeout(STEP_TIMEOUT, future).await {
        Ok(value) => value,
        Err(elapsed) => panic!("{elapsed} after {STEP_TIMEOUT:?} waiting for: {what}"),
    }
}

/// The guard above is only worth having if it actually fires, so this holds
/// it to that. `start_paused` advances the clock as soon as the runtime is
/// idle, which is what keeps a test about a ten-second timeout instant.
#[tokio::test(start_paused = true)]
async fn a_step_that_never_completes_fails_instead_of_hanging() {
    let outcome = tokio::spawn(async {
        within("a step that never completes", std::future::pending::<()>()).await;
    })
    .await;

    let error = outcome.expect_err("a step that never completes must not report success");
    assert!(
        error.is_panic(),
        "the step must fail loudly, not end some other way"
    );
}

/// A tiny in-process WebSocket server standing in for a venue — the same
/// shape `crates/feed/tests/live_engine.rs` already uses for the generic
/// dial/subscribe/publish engine, reproduced here (not imported: it is
/// private to that crate's own test binary) for this crate's own wiring
/// tests.
struct FakeServer {
    listener: TcpListener,
    addr: std::net::SocketAddr,
}

impl FakeServer {
    async fn bind() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        Self { listener, addr }
    }

    fn ws_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    async fn accept(&self) -> FakeConn {
        let (stream, _peer) = self.listener.accept().await.unwrap();
        let ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        FakeConn { ws }
    }
}

struct FakeConn {
    ws: WebSocketStream<TcpStream>,
}

impl FakeConn {
    async fn recv_text(&mut self) -> String {
        loop {
            match self.ws.next().await {
                Some(Ok(WsMessage::Text(text))) => return text.to_string(),
                Some(Ok(_)) => {}
                Some(Err(error)) => panic!("server-side read error: {error}"),
                None => panic!("client disconnected before sending the expected frame"),
            }
        }
    }

    async fn send_text(&mut self, text: &str) {
        self.ws.send(WsMessage::text(text)).await.unwrap();
    }
}

/// A deliberately trivial wire format, distinct from `senken_feed::okx`'s
/// real one and from `live_engine.rs`'s own test protocol only in that a
/// price frame carries its own timestamp — this suite needs two ticks in
/// two different minutes to close a real bar.
struct TestProtocol {
    url: String,
}

impl senken_feed::VenueProtocol for TestProtocol {
    fn url(&self) -> &str {
        &self.url
    }

    fn venue(&self) -> &'static str {
        TEST_SOURCE
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(format!("SUB {}", instrument.symbol()))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(format!("UNSUB {}", instrument.symbol()))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        // "PRICE <symbol> <price> <scale> <ts_millis>" or
        // "QUOTE <symbol> <bid> <ask> <scale> <ts_millis>"
        let mut parts = text.split_whitespace();
        let kind = parts.next();
        let Some(symbol) = parts.next() else {
            return Vec::new();
        };
        let Some(first) = parts.next().and_then(|p| p.parse::<i64>().ok()) else {
            return Vec::new();
        };
        let second = if kind == Some("QUOTE") {
            let Some(value) = parts.next().and_then(|p| p.parse::<i64>().ok()) else {
                return Vec::new();
            };
            value
        } else {
            0
        };
        let Some(price_scale) = parts.next().and_then(|p| p.parse::<u8>().ok()) else {
            return Vec::new();
        };
        let Some(ts_millis) = parts.next().and_then(|p| p.parse::<i64>().ok()) else {
            return Vec::new();
        };
        let Ok(instrument) = InstrumentId::new(TEST_SOURCE, symbol) else {
            return Vec::new();
        };
        let Some(ts) = UnixNanos::from_millis(ts_millis) else {
            return Vec::new();
        };
        match kind {
            Some("PRICE") => vec![(
                instrument,
                LiveUpdate::Price(PriceUpdate {
                    ts,
                    price: first,
                    price_scale,
                    qty: senken_series::Volume::Real(0),
                    qty_scale: 0,
                }),
            )],
            Some("QUOTE") => QuoteUpdate::new(
                ts,
                (first, price_scale),
                (second, price_scale),
                (0, 0),
                (0, 0),
            )
            .map(|quote| vec![(instrument, LiveUpdate::Quote(quote))])
            .unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

/// Builds a real `WsVenueConnector`/`SubscriptionPool` pair dialling
/// `server`, following `senken_feed::WsVenueConnector`'s own documented
/// two-step construction — the exact plumbing `crate::feed::build_feed_pools`
/// builds for OKX, minus OKX itself.
fn fake_pool(server: &FakeServer) -> HashMap<String, SubscriptionPool> {
    let protocol = TestProtocol {
        url: server.ws_url(),
    };
    let group = LimitGroup::new(TEST_SOURCE);
    let connector = senken_feed::WsVenueConnector::new(protocol, group);
    let pool = SubscriptionPool::new(TEST_SOURCE, connector.clone());
    connector.bind_pool(pool.clone());
    HashMap::from([(TEST_SOURCE.to_owned(), pool)])
}

async fn admin_token(addr: std::net::SocketAddr) -> String {
    let response = post_json(
        format!("http://{addr}/api/login"),
        serde_json::json!({ "email": DEFAULT_ADMIN_EMAIL, "password": ADMIN_TEST_PASSWORD }),
    )
    .await;
    body_json(response).await["token"]
        .as_str()
        .unwrap()
        .to_owned()
}

async fn ws_ticket(addr: std::net::SocketAddr, token: &str) -> String {
    let response = post_json_auth(
        format!("http://{addr}/api/ws/ticket"),
        token,
        serde_json::json!({}),
    )
    .await;
    body_json(response).await["ticket"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// A user with exactly the grants a real "Alerts User" role carries —
/// mirrors `alert_handlers`'s own test helper of the same shape.
fn alerts_user(
    identity: &senken_identity::IdentityStore,
    admin: &senken_identity::AuthenticatedUser,
    email: &str,
) -> senken_identity::AuthenticatedUser {
    use senken_acl::{Action, Grant, Resource, Scope};
    let user_id = identity
        .create_user(admin, email, "Alerts User", Some("a very long password"))
        .unwrap();
    for action in [Action::View, Action::Create, Action::Delete] {
        identity
            .grant_direct(
                admin,
                user_id,
                Grant::new(action, Resource::Alert, Scope::Own),
            )
            .unwrap();
    }
    let (_uid, token) = identity.login(email, "a very long password").unwrap();
    identity.resolve_session(token.reveal()).unwrap().unwrap()
}

/// A source this build cannot stream must be answered, not ignored: silence
/// is indistinguishable from "the feed is fine, nothing has traded yet", and
/// a client that cannot tell them apart shows a live state for a feed that
/// is not running.
#[tokio::test(flavor = "multi_thread")]
async fn a_topic_with_no_feed_is_answered_as_unsupported() {
    let server = FakeServer::bind().await;
    let feed_pools = fake_pool(&server);
    let (_runtime_dir, runtime) = temp_empty_runtime();
    let (handle, _identity, _dir) = serve_unfenced_test_server_with_feed(runtime, feed_pools).await;
    let addr = handle.local_addr();
    let token = admin_token(addr).await;
    let ticket = ws_ticket(addr, &token).await;

    let (mut client, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws?ticket={ticket}"))
            .await
            .unwrap();
    let _connected = client.next().await.unwrap().unwrap();

    client
        .send(WsMessage::text(
            r#"{"type":"subscribe","topic":"no-such-venue:BTCUSDT"}"#,
        ))
        .await
        .unwrap();

    let frame: serde_json::Value =
        serde_json::from_str(client.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(frame["type"], "unsupported");
    assert_eq!(frame["topic"], "no-such-venue:BTCUSDT");
}

/// A book topic is gated by its own registry, entirely separate from the
/// price/quote pool a source's `feed_pools` entry serves: `test-venue` has a
/// live pool (this suite's own fake server proves it can stream price and
/// quote topics), but no registered `senken_subscription::BookSource`, so a
/// book subscription for it must still come back `unsupported` rather than
/// falling back to "has a feed pool, so it must have everything".
#[tokio::test(flavor = "multi_thread")]
async fn a_book_topic_for_a_source_with_no_registered_book_source_is_unsupported() {
    let server = FakeServer::bind().await;
    let feed_pools = fake_pool(&server);
    let (_runtime_dir, runtime) = temp_empty_runtime();
    let (handle, _identity, _dir) = serve_unfenced_test_server_with_feed(runtime, feed_pools).await;
    let addr = handle.local_addr();
    let token = admin_token(addr).await;
    let ticket = ws_ticket(addr, &token).await;

    let (mut client, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws?ticket={ticket}"))
            .await
            .unwrap();
    let _connected = client.next().await.unwrap().unwrap();

    client
        .send(WsMessage::text(
            r#"{"type":"subscribe","topic":"book:test-venue:BTCUSDT"}"#,
        ))
        .await
        .unwrap();

    let frame: serde_json::Value =
        serde_json::from_str(client.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(frame["type"], "unsupported");
    assert_eq!(frame["topic"], "book:test-venue:BTCUSDT");

    drop(client);
    drop(server);
    handle.shutdown().await.unwrap();
}

/// a WS client subscribing to a live topic leases the pool
/// exactly the way a chart pane does, and closing the connection — without
/// ever sending `Unsubscribe` — releases it, unprompted.
#[tokio::test(flavor = "multi_thread")]
async fn a_ws_subscription_streams_ticks_and_releases_on_disconnect() {
    let server = FakeServer::bind().await;
    let feed_pools = fake_pool(&server);
    let (_runtime_dir, runtime) = temp_empty_runtime();
    let (handle, _identity, _dir) = serve_unfenced_test_server_with_feed(runtime, feed_pools).await;
    let addr = handle.local_addr();
    let token = admin_token(addr).await;
    let ticket = ws_ticket(addr, &token).await;

    let server_task = tokio::spawn(async move {
        let mut conn = server.accept().await;
        let sub = conn.recv_text().await;
        (server, conn, sub)
    });

    // `Box::pin`: the handshake future is ~20 KB, which `clippy::large_futures`
    // rightly objects to once it is nested inside a timeout.
    let (mut client, _resp) = within(
        "the WebSocket handshake",
        Box::pin(tokio_tungstenite::connect_async(format!(
            "ws://{addr}/api/ws?ticket={ticket}"
        ))),
    )
    .await
    .unwrap();
    let _ = within("the `connected` frame", client.next())
        .await
        .unwrap()
        .unwrap();

    client
        .send(WsMessage::text(
            r#"{"type":"subscribe","topic":"test-venue:BTCUSDT"}"#,
        ))
        .await
        .unwrap();
    // "subscribed" ack, sent before the lease necessarily completes.
    let ack: serde_json::Value = serde_json::from_str(
        within("the `subscribed` ack", client.next())
            .await
            .unwrap()
            .unwrap()
            .to_text()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(ack["type"], "subscribed");

    let (server, mut conn, sub) = within("the fake venue to accept and receive SUB", server_task)
        .await
        .unwrap();
    assert_eq!(
        sub, "SUB BTCUSDT",
        "the WS subscribe must have leased the pool, dialling the fake venue"
    );

    conn.send_text("PRICE BTCUSDT 78146 2 1000").await;
    let price: serde_json::Value = serde_json::from_str(
        within("the price frame to reach the client", client.next())
            .await
            .unwrap()
            .unwrap()
            .to_text()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(price["type"], "price");
    assert_eq!(price["topic"], "test-venue:BTCUSDT");
    assert_eq!(price["price"], 78146);
    assert_eq!(price["price_scale"], 2);

    // Ungraceful close — no `Unsubscribe` frame at all, the same as a
    // browser tab or laptop lid just closing.
    drop(client);

    let unsub = tokio::time::timeout(Duration::from_secs(5), conn.recv_text())
        .await
        .expect("the dropped connection must release its lease without anyone calling anything");
    assert_eq!(unsub, "UNSUB BTCUSDT");

    drop(conn);
    drop(server);
    handle.shutdown().await.unwrap();
}

/// Quote topics are namespaced from last-trade topics and forward both sides
/// with their shared integer scale. The fake protocol proves the complete WS
/// path without a live venue connection.
#[tokio::test(flavor = "multi_thread")]
async fn a_ws_quote_subscription_streams_bid_and_ask() {
    let server = FakeServer::bind().await;
    let feed_pools = fake_pool(&server);
    let (_runtime_dir, runtime) = temp_empty_runtime();
    let (handle, _identity, _dir) = serve_unfenced_test_server_with_feed(runtime, feed_pools).await;
    let addr = handle.local_addr();
    let token = admin_token(addr).await;
    let ticket = ws_ticket(addr, &token).await;

    let server_task = tokio::spawn(async move {
        let mut conn = server.accept().await;
        let sub = conn.recv_text().await;
        (server, conn, sub)
    });
    let (mut client, _response) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws?ticket={ticket}"))
            .await
            .unwrap();
    let _connected = client.next().await.unwrap().unwrap();
    client
        .send(WsMessage::text(
            r#"{"type":"subscribe","topic":"quote:test-venue:BTCUSDT"}"#,
        ))
        .await
        .unwrap();
    let ack: serde_json::Value =
        serde_json::from_str(client.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(ack["type"], "subscribed");

    let (server, mut conn, sub) = server_task.await.unwrap();
    assert_eq!(sub, "SUB BTCUSDT");
    conn.send_text("QUOTE BTCUSDT 779955 779956 1 1000").await;
    let quote: serde_json::Value =
        serde_json::from_str(client.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(quote["type"], "quote");
    assert_eq!(quote["topic"], "quote:test-venue:BTCUSDT");
    assert_eq!(quote["bid"], 779_955);
    assert_eq!(quote["ask"], 779_956);
    assert_eq!(quote["price_scale"], 1);

    drop(client);
    let unsub = tokio::time::timeout(Duration::from_secs(5), conn.recv_text())
        .await
        .expect("dropping a quote subscriber must release its lease");
    assert_eq!(unsub, "UNSUB BTCUSDT");
    drop(conn);
    drop(server);
    handle.shutdown().await.unwrap();
}

/// The property this file exists to prove, through the real server rather
/// than only through
/// `senken_alerts::runner`'s own unit test: an alert leases its instrument
/// independently of any chart, keeps running after a chart pane sharing the
/// same instrument disconnects, and its fire is recorded and visible over
/// HTTP.
#[tokio::test(flavor = "multi_thread")]
async fn an_alert_outlives_its_chart_pane_and_records_a_fire_over_http() {
    let server = FakeServer::bind().await;
    let feed_pools = fake_pool(&server);
    let (_runtime_dir, runtime) = temp_empty_runtime();
    let (handle, identity, _dir) = serve_unfenced_test_server_with_feed(runtime, feed_pools).await;
    let addr = handle.local_addr();

    let (_uid, admin_session) = identity
        .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
        .unwrap();
    let admin = identity
        .resolve_session(admin_session.reveal())
        .unwrap()
        .unwrap();
    let alice = alerts_user(&identity, &admin, "alice-live@example.com");
    let (_uid2, alice_token) = identity
        .login("alice-live@example.com", "a very long password")
        .unwrap();
    let alice_token = alice_token.reveal().to_owned();
    drop(alice);

    // The alert is created first, so its own lease is what dials the fake
    // venue — the "chart" below shares the same, already-open subscription.
    let server_task = tokio::spawn(async move {
        let mut conn = server.accept().await;
        let sub = conn.recv_text().await;
        (server, conn, sub)
    });

    let create = post_json_auth(
        format!("http://{addr}/api/alerts"),
        &alice_token,
        serde_json::json!({
            "instrument": "test-venue:BTCUSDT",
            "timeframe": "1m",
            "indicator": { "name": "Sma", "params": r#"{"period":1}"# },
            "condition": { "field": "Value", "comparator": "GreaterThan", "threshold": 100.0 },
        }),
    )
    .await;
    assert_eq!(create.status(), reqwest::StatusCode::CREATED);
    let alert_id = body_json(create).await["id"].as_str().unwrap().to_owned();

    let (server, mut conn, sub) = server_task.await.unwrap();
    assert_eq!(
        sub, "SUB BTCUSDT",
        "creating the alert must have leased the pool immediately"
    );

    // A "chart pane" leases the exact same instrument over the WS endpoint
    // — the pool must not dial a second connection (`sub` above was the
    // only subscribe frame the fake venue will ever see).
    let ticket = ws_ticket(addr, &alice_token).await;
    let (mut pane, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws?ticket={ticket}"))
            .await
            .unwrap();
    let _ = pane.next().await.unwrap().unwrap(); // "connected"
    pane.send(WsMessage::text(
        r#"{"type":"subscribe","topic":"test-venue:BTCUSDT"}"#,
    ))
    .await
    .unwrap();
    let ack: serde_json::Value =
        serde_json::from_str(pane.next().await.unwrap().unwrap().to_text().unwrap()).unwrap();
    assert_eq!(ack["type"], "subscribed");

    // The chart closes. The alert must keep running — nothing here ever
    // touches the alert's own lease.
    drop(pane);
    let no_unsub_yet = tokio::time::timeout(Duration::from_millis(300), conn.recv_text()).await;
    assert!(
        no_unsub_yet.is_err(),
        "the alert's own lease must keep the venue subscription alive after the chart pane disconnects"
    );

    // Two ticks across a minute boundary: the first opens (and, with only
    // one tick in it, also closes at) minute 0 with price 150.00; the
    // second, in minute 1, is what closes minute 0's bucket. Only the
    // *closed* bar's value (150.00) is ever compared against the
    // threshold — never the still-forming minute 1 tick (5.00, which is
    // below the threshold and would not have fired at all).
    // A watch channel only ever holds the *latest* value, so two publishes
    // made back-to-back before anything reads the channel would coalesce
    // into one and the first (bucket-opening) tick would be lost — the
    // same reason `senken_alerts::runner`'s own test interleaves a sleep
    // between its two ticks. This is what gives the alert's own task room
    // to actually observe both.
    conn.send_text("PRICE BTCUSDT 150 2 0").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    conn.send_text("PRICE BTCUSDT 5 2 65000").await;

    let fired = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let record = body_json(
                get_auth(format!("http://{addr}/api/alerts/{alert_id}"), &alice_token).await,
            )
            .await;
            if record["fire_count"].as_u64().unwrap_or(0) > 0 {
                return record;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the alert must fire well within this generous timeout");

    assert_eq!(fired["fire_count"], 1);
    assert!(
        (fired["last_fired_value"].as_f64().unwrap() - 150.0).abs() < 1e-9,
        "must fire on the closed bar's own close (150.0), not the still-forming tick (5.0): {fired}"
    );

    // Deleting the alert must stop it — no further lease, so a later
    // release the fake venue could observe is provable by shutting it down
    // cleanly, which only succeeds if nothing is left holding the socket.
    let delete = reqwest::Client::new()
        .delete(format!("http://{addr}/api/alerts/{alert_id}"))
        .header("authorization", format!("Bearer {alice_token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(delete.status(), reqwest::StatusCode::NO_CONTENT);

    let unsub = tokio::time::timeout(Duration::from_secs(5), conn.recv_text())
        .await
        .expect("deleting the alert must release its lease");
    assert_eq!(unsub, "UNSUB BTCUSDT");

    drop(conn);
    drop(server);
    handle.shutdown().await.unwrap();
}

/// The refresh cadence every book test in this module runs its registry at.
///
/// Fast enough that three refreshes land inside a fraction of a second, and
/// deliberately no faster: these tests share a test binary with timing-
/// sensitive live-feed tests, and a hotter loop spends the machine's CPU on
/// nothing the assertions need. Every book test here waits on a frame
/// actually arriving, never on this elapsing, so the figure only sets how
/// long they take — not whether they pass.
const BOOK_TEST_INTERVAL: Duration = Duration::from_millis(40);

/// A fake venue book-depth endpoint: a fresh, distinct best bid on every
/// call, and a call counter published on a `watch` channel so a test can
/// wait for the poll loop to have actually run rather than sleep towards it
/// (`asked_at_least`, mirroring `senken_subscription::book_session`'s own
/// test fake of the same shape — reproduced here because it is private to
/// that crate's own test module).
struct ScriptedBookSource {
    calls: watch::Sender<usize>,
}

impl ScriptedBookSource {
    fn new() -> Arc<Self> {
        let (calls, _) = watch::channel(0);
        Arc::new(Self { calls })
    }

    /// Resolves once the fake venue has been asked at least `n` times —
    /// establishes the precondition a test's assertion is about, rather
    /// than sleeping towards it.
    async fn asked_at_least(&self, n: usize) {
        let mut calls = self.calls.subscribe();
        while *calls.borrow_and_update() < n {
            calls
                .changed()
                .await
                .expect("the counter outlives the test");
        }
    }
}

#[async_trait::async_trait]
impl BookSource for ScriptedBookSource {
    fn source_id(&self) -> &'static str {
        TEST_SOURCE
    }

    async fn book_snapshot(
        &self,
        _symbol: &SourceSymbol,
        _depth: usize,
    ) -> Result<BookSnapshot, SourceError> {
        let mut call = 0usize;
        self.calls.send_modify(|count| {
            *count += 1;
            call = *count;
        });
        // A fresh best bid on every call: the property under test is that a
        // client sees the price *change* across frames, not merely that a
        // frame arrives — the fetch-once behaviour this whole change
        // replaces could produce identical repeats forever and still pass a
        // weaker test.
        let bid = 100 + i64::try_from(call).expect("test call counts stay well under i64::MAX");
        Ok(BookSnapshot::new(
            UnixNanos::EPOCH,
            vec![BookLevel {
                price: bid,
                size: 1,
            }],
            2,
            0,
            vec![BookLevel {
                price: bid + 1,
                size: 1,
            }],
            2,
            0,
        )
        .expect("bid and ask sides share one scale"))
    }
}

/// Reads WS frames until one has `"type": kind`, discarding anything else —
/// `Pending` book states forward nothing, so a subscriber's very first
/// message may be an unrelated frame rather than the one under test.
async fn next_frame_of_type(
    client: &mut WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    kind: &str,
) -> serde_json::Value {
    loop {
        let text = client
            .next()
            .await
            .expect("the connection stays open")
            .expect("not a transport error")
            .to_text()
            .expect("a text frame")
            .to_owned();
        let frame: serde_json::Value = serde_json::from_str(&text).unwrap();
        if frame["type"] == kind {
            return frame;
        }
    }
}

/// The property this whole change exists for: a client subscribes to a
/// `book:` topic once and the depth keeps refreshing on its own. A test that
/// only checked the first frame would pass unchanged against the old
/// fetch-once-per-subscribe behaviour — this asserts at least three `book`
/// frames arrive from one `subscribe`, and that the prices they carry
/// actually differ, which a fetch-once implementation could never produce
/// (see `ScriptedBookSource`).
#[tokio::test(flavor = "multi_thread")]
async fn a_book_subscription_keeps_yielding_fresh_frames_without_the_client_asking_again() {
    use crate::bars_handlers::test_support::{TEST_SYMBOL, runtime_with_fake_venue_serving};

    let runtime_dir = tempfile::TempDir::new().unwrap();
    let source = ScriptedBookSource::new();
    let (runtime, _bar_source) = runtime_with_fake_venue_serving(
        runtime_dir.path(),
        Some(Arc::clone(&source) as Arc<dyn BookSource>),
    );
    let registry = Arc::new(BookSessionRegistry::new(20).with_interval(BOOK_TEST_INTERVAL));

    let (handle, _identity, _dir) =
        serve_unfenced_test_server_with_book(runtime, HashMap::new(), Arc::clone(&registry)).await;
    let addr = handle.local_addr();
    let token = admin_token(addr).await;
    let ticket = ws_ticket(addr, &token).await;

    let (mut client, _resp) = within(
        "the WebSocket handshake",
        Box::pin(tokio_tungstenite::connect_async(format!(
            "ws://{addr}/api/ws?ticket={ticket}"
        ))),
    )
    .await
    .unwrap();
    let _ = within("the `connected` frame", client.next())
        .await
        .unwrap()
        .unwrap();

    client
        .send(WsMessage::text(format!(
            r#"{{"type":"subscribe","topic":"book:{TEST_SOURCE}:{TEST_SYMBOL}"}}"#
        )))
        .await
        .unwrap();
    let ack = within(
        "the `subscribed` ack",
        next_frame_of_type(&mut client, "subscribed"),
    )
    .await;
    assert_eq!(ack["topic"], format!("book:{TEST_SOURCE}:{TEST_SYMBOL}"));

    let mut prices = Vec::new();
    while prices.len() < 3 {
        let frame = within("a `book` frame", next_frame_of_type(&mut client, "book")).await;
        prices.push(frame["bids"][0]["price"].as_i64().unwrap());
    }

    assert!(
        prices.windows(2).any(|pair| pair[0] != pair[1]),
        "three `book` frames arrived but every price was identical — \
         not proof of a live poll, only of repeated delivery: {prices:?}"
    );

    drop(client);
    handle.shutdown().await.unwrap();
}

/// `unsubscribe` on a book topic must stop the frames themselves, not only
/// acknowledge the request. The negative window below is only meaningful
/// once the `unsubscribed` ack has actually been seen — per `AGENTS.md`, a
/// test must establish the state it is about rather than race towards it.
#[tokio::test(flavor = "multi_thread")]
async fn unsubscribing_from_a_book_topic_stops_further_frames() {
    use crate::bars_handlers::test_support::{TEST_SYMBOL, runtime_with_fake_venue_serving};

    let runtime_dir = tempfile::TempDir::new().unwrap();
    let source = ScriptedBookSource::new();
    let (runtime, _bar_source) = runtime_with_fake_venue_serving(
        runtime_dir.path(),
        Some(Arc::clone(&source) as Arc<dyn BookSource>),
    );
    let interval = BOOK_TEST_INTERVAL;
    let registry = Arc::new(BookSessionRegistry::new(20).with_interval(interval));

    let (handle, _identity, _dir) =
        serve_unfenced_test_server_with_book(runtime, HashMap::new(), registry).await;
    let addr = handle.local_addr();
    let token = admin_token(addr).await;
    let ticket = ws_ticket(addr, &token).await;

    let (mut client, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws?ticket={ticket}"))
            .await
            .unwrap();
    let _ = within("the `connected` frame", client.next())
        .await
        .unwrap()
        .unwrap();

    let topic = format!("book:{TEST_SOURCE}:{TEST_SYMBOL}");
    client
        .send(WsMessage::text(format!(
            r#"{{"type":"subscribe","topic":"{topic}"}}"#
        )))
        .await
        .unwrap();
    let ack = within(
        "the `subscribed` ack",
        next_frame_of_type(&mut client, "subscribed"),
    )
    .await;
    assert_eq!(ack["topic"], topic);

    // Establish that frames are actually flowing before proving they stop —
    // the negative assertion below is only worth having once this is true.
    let _ = within(
        "the first `book` frame",
        next_frame_of_type(&mut client, "book"),
    )
    .await;

    client
        .send(WsMessage::text(format!(
            r#"{{"type":"unsubscribe","topic":"{topic}"}}"#
        )))
        .await
        .unwrap();
    let unsub_ack = within(
        "the `unsubscribed` ack",
        next_frame_of_type(&mut client, "unsubscribed"),
    )
    .await;
    assert_eq!(unsub_ack["topic"], topic);

    // Only meaningful now that the ack (proof the forwarder task was
    // aborted) has actually been seen: a window several polls wide with no
    // `book` frame is a real negative, not a guess about scheduler timing.
    let window = interval * 10;
    let further = tokio::time::timeout(window, next_frame_of_type(&mut client, "book")).await;
    assert!(
        further.is_err(),
        "a `book` frame arrived after unsubscribe: {further:?}"
    );

    drop(client);
    handle.shutdown().await.unwrap();
}

/// Two WS connections watching the same instrument's book must share one
/// poll loop — checked directly against the registry (kept via a clone of
/// the same `Arc` the server itself runs on) rather than inferred from call
/// counts.
#[tokio::test(flavor = "multi_thread")]
async fn two_connections_on_the_same_book_share_one_poll_loop() {
    use crate::bars_handlers::test_support::{TEST_SYMBOL, runtime_with_fake_venue_serving};

    let runtime_dir = tempfile::TempDir::new().unwrap();
    let source = ScriptedBookSource::new();
    let (runtime, _bar_source) = runtime_with_fake_venue_serving(
        runtime_dir.path(),
        Some(Arc::clone(&source) as Arc<dyn BookSource>),
    );
    let registry = Arc::new(BookSessionRegistry::new(20).with_interval(BOOK_TEST_INTERVAL));

    let (handle, _identity, _dir) =
        serve_unfenced_test_server_with_book(runtime, HashMap::new(), Arc::clone(&registry)).await;
    let addr = handle.local_addr();
    let topic = format!("book:{TEST_SOURCE}:{TEST_SYMBOL}");

    let client_a = Box::pin(within(
        "connection A's first `book` frame",
        subscribe_and_wait_for_a_book_frame(addr, &topic),
    ))
    .await;
    let client_b = Box::pin(within(
        "connection B's first `book` frame",
        subscribe_and_wait_for_a_book_frame(addr, &topic),
    ))
    .await;

    assert_eq!(
        registry.live_sessions().await,
        1,
        "two connections on the same instrument must share one poll loop"
    );
    // Both connections having already seen a live snapshot proves the fake
    // venue was actually asked, not just that the registry says one session
    // exists before any work has happened.
    source.asked_at_least(1).await;

    drop(client_a);
    drop(client_b);
    handle.shutdown().await.unwrap();
}

/// Logs in, subscribes `topic`, and waits for its first `book` frame —
/// shared by [`two_connections_on_the_same_book_share_one_poll_loop`]'s two
/// connections.
async fn subscribe_and_wait_for_a_book_frame(
    addr: std::net::SocketAddr,
    topic: &str,
) -> WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>> {
    let token = admin_token(addr).await;
    let ticket = ws_ticket(addr, &token).await;
    let (mut client, _resp) =
        tokio_tungstenite::connect_async(format!("ws://{addr}/api/ws?ticket={ticket}"))
            .await
            .unwrap();
    let _ = client.next().await.unwrap().unwrap();
    client
        .send(WsMessage::text(format!(
            r#"{{"type":"subscribe","topic":"{topic}"}}"#
        )))
        .await
        .unwrap();
    let _ = next_frame_of_type(&mut client, "book").await;
    client
}
