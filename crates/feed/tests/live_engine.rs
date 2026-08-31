//! the required tests, run against a real `WsVenueConnector` and
//! `SubscriptionPool` talking over a real (loopback) TCP socket — never a
//! real venue's own "everything else must be testable
//! against a fake" constraint. Only [`senken_feed::okx`]'s unit tests touch
//! anything OKX-specific; this file exercises the generic engine with a
//! trivial made-up wire format.

use futures::{SinkExt, StreamExt};
use senken_core::UnixNanos;
use senken_marketdata::InstrumentId;
use senken_subscription::{ConnectionError, PriceUpdate, SubscriptionPool};
use senken_venue::LimitGroup;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

/// A tiny in-process WebSocket server standing in for a venue.
struct FakeServer {
    listener: TcpListener,
    addr: std::net::SocketAddr,
}

impl FakeServer {
    async fn bind() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("binding a loopback port must not fail in a test sandbox");
        let addr = listener
            .local_addr()
            .expect("a bound listener always has a local address");
        Self { listener, addr }
    }

    fn ws_url(&self) -> String {
        format!("ws://{}", self.addr)
    }

    /// Waits for and completes the next incoming WebSocket handshake.
    async fn accept(&self) -> FakeConn {
        let (stream, _peer) = self
            .listener
            .accept()
            .await
            .expect("accepting a loopback connection must not fail in a test sandbox");
        let ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("the client always completes a plain WS handshake in these tests");
        FakeConn { ws }
    }
}

/// The server's own end of one accepted connection.
struct FakeConn {
    ws: WebSocketStream<TcpStream>,
}

impl FakeConn {
    /// Waits for the next text frame, ignoring anything else (a real venue
    /// server might send pings; this test double does not need to model
    /// that to prove the client's subscribe/publish wiring).
    async fn recv_text(&mut self) -> String {
        loop {
            match self.ws.next().await {
                Some(Ok(Message::Text(text))) => return text.to_string(),
                Some(Ok(_)) => {}
                Some(Err(error)) => panic!("server-side read error: {error}"),
                None => panic!("client disconnected before sending the expected frame"),
            }
        }
    }

    async fn send_text(&mut self, text: &str) {
        self.ws
            .send(Message::text(text))
            .await
            .expect("sending to a freshly accepted loopback socket must not fail");
    }

    /// Drops the TCP connection outright — not a graceful WS close — the
    /// same abrupt failure a venue's own connection drop, a network blip or
    /// a proxy restart would look like to the client.
    fn hang_up(self) {
        drop(self.ws);
    }
}

/// A deliberately trivial wire format — plain space-separated text, nothing
/// like any real venue — so these tests exercise only the generic
/// dial/subscribe/publish/reconnect engine, never anything OKX-specific
/// (that lives in `senken_feed::okx`'s own unit tests instead).
struct TestProtocol {
    url: String,
}

impl senken_feed::VenueProtocol for TestProtocol {
    fn url(&self) -> &str {
        &self.url
    }

    fn venue(&self) -> &'static str {
        "test-venue"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(format!("SUB {}", instrument.symbol()))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(format!("UNSUB {}", instrument.symbol()))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, PriceUpdate)> {
        // "PRICE <symbol> <price> <scale>"
        let mut parts = text.split_whitespace();
        if parts.next() != Some("PRICE") {
            return Vec::new();
        }
        let Some(symbol) = parts.next() else {
            return Vec::new();
        };
        let Some(price) = parts.next().and_then(|p| p.parse::<i64>().ok()) else {
            return Vec::new();
        };
        let Some(price_scale) = parts.next().and_then(|p| p.parse::<u8>().ok()) else {
            return Vec::new();
        };
        let Ok(instrument) = InstrumentId::new("test-venue", symbol) else {
            return Vec::new();
        };
        vec![(
            instrument,
            PriceUpdate {
                ts: UnixNanos::EPOCH,
                price,
                price_scale,
                qty: 0,
                qty_scale: 0,
            },
        )]
    }
}

fn instrument(symbol: &str) -> InstrumentId {
    InstrumentId::new("test-venue", symbol).unwrap()
}

/// Builds a connector and its pool together, resolving the two-step
/// construction [`senken_feed::WsVenueConnector`]'s own docs describe.
fn connector_and_pool(
    url: String,
) -> (
    senken_feed::WsVenueConnector<TestProtocol>,
    SubscriptionPool,
) {
    let protocol = TestProtocol { url };
    let group = LimitGroup::new("test-venue");
    let connector = senken_feed::WsVenueConnector::new(protocol, group);
    let pool = SubscriptionPool::new("test-venue", connector.clone());
    connector.bind_pool(pool.clone());
    (connector, pool)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lease_receives_updates_for_its_instrument_and_not_for_others() {
    let server = FakeServer::bind().await;
    let (_connector, pool) = connector_and_pool(server.ws_url());

    let server_task = tokio::spawn(async move {
        let mut conn = server.accept().await;
        let first_sub = conn.recv_text().await;
        let second_sub = conn.recv_text().await;
        (conn, first_sub, second_sub)
    });

    let btc = pool.lease(instrument("BTCUSDT")).await.unwrap();
    let eth = pool.lease(instrument("ETHUSDT")).await.unwrap();

    let (mut conn, first_sub, second_sub) = server_task.await.unwrap();
    assert_eq!(first_sub, "SUB BTCUSDT");
    assert_eq!(second_sub, "SUB ETHUSDT");

    let mut btc_updates = btc.updates();
    let eth_updates = eth.updates();

    conn.send_text("PRICE BTCUSDT 78146 2").await;
    btc_updates.changed().await.unwrap();
    assert_eq!(
        btc_updates.borrow().map(|u| u.price),
        Some(78_146),
        "the leaseholder for BTCUSDT must see the price published for it"
    );
    assert_eq!(
        *eth_updates.borrow(),
        None,
        "a price published for BTCUSDT must never reach ETHUSDT's leaseholder"
    );

    drop(btc);
    drop(eth);
    pool.flush().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn dropping_the_last_lease_stops_the_venue_subscription() {
    let server = FakeServer::bind().await;
    let (_connector, pool) = connector_and_pool(server.ws_url());

    let server_task = tokio::spawn(async move {
        let mut conn = server.accept().await;
        let sub = conn.recv_text().await;
        (conn, sub)
    });

    let first = pool.lease(instrument("SOLUSDT")).await.unwrap();
    let second = pool.lease(instrument("SOLUSDT")).await.unwrap();
    let (mut conn, sub) = server_task.await.unwrap();
    assert_eq!(sub, "SUB SOLUSDT");

    drop(first);
    pool.flush().await;
    let too_early =
        tokio::time::timeout(std::time::Duration::from_millis(200), conn.recv_text()).await;
    assert!(
        too_early.is_err(),
        "a non-last release must not unsubscribe"
    );

    drop(second);
    pool.flush().await;
    let unsub = conn.recv_text().await;
    assert_eq!(unsub, "UNSUB SOLUSDT");
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnect_resubscribes_exactly_what_the_pool_says_is_leased() {
    let server = FakeServer::bind().await;
    let (_connector, pool) = connector_and_pool(server.ws_url());

    let server_task = tokio::spawn(async move {
        let conn = server.accept().await;
        // First subscribe, over the first (soon-to-die) socket.
        let mut conn = conn;
        let first_sub = conn.recv_text().await;
        conn.hang_up(); // the venue's own connection drops, mid-lease

        // The client must redial and replay its subscribe on the new
        // socket, without this test ever calling `pool.lease` again — the
        // pool is the sole authority on what is currently leased, not the connection's own memory of what it once subscribed.
        let mut second_conn = server.accept().await;
        let resub = second_conn.recv_text().await;
        (first_sub, resub)
    });

    let lease = pool.lease(instrument("BTCUSDT")).await.unwrap();

    let (first_sub, resub) = server_task.await.unwrap();
    assert_eq!(first_sub, "SUB BTCUSDT");
    assert_eq!(
        resub, "SUB BTCUSDT",
        "reconnect must replay exactly the instrument still leased, from the pool, not from the dead connection's own memory"
    );

    drop(lease);
}

#[tokio::test(flavor = "multi_thread")]
async fn reconnect_never_resubscribes_an_instrument_already_released() {
    // The stronger version of the above: release the lease *while the
    // connection is down*, then bring the server back up. The replay must
    // reflect the pool's current state at reconnect time, not whatever was
    // leased at the moment the socket dropped.
    let server = FakeServer::bind().await;
    let (_connector, pool) = connector_and_pool(server.ws_url());

    let server_task = tokio::spawn(async move {
        let mut conn = server.accept().await;
        let first_sub = conn.recv_text().await;
        conn.hang_up();
        let mut second_conn = server.accept().await;
        // Prove nothing at all is replayed: any frame arriving here would
        // be exactly the bug this test exists to catch.
        let anything_else = tokio::time::timeout(
            std::time::Duration::from_millis(300),
            second_conn.recv_text(),
        )
        .await;
        (first_sub, anything_else.is_err())
    });

    let lease = pool.lease(instrument("ETHUSDT")).await.unwrap();
    // Released before the server (deliberately) brings the second socket
    // up on its own schedule — by the time reconnect happens, the pool no
    // longer leases anything at all.
    drop(lease);
    pool.flush().await;

    let (first_sub, nothing_replayed) = server_task.await.unwrap();
    assert_eq!(first_sub, "SUB ETHUSDT");
    assert!(
        nothing_replayed,
        "reconnect must not resubscribe an instrument that was released while disconnected"
    );
}
