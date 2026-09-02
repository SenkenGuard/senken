//! [`WsVenueConnection`] — the generic engine: dial, read, decode, publish,
//! and reconnect with jitter. Everything venue-specific lives behind
//! [`VenueProtocol`] instead; this module is tested entirely against a fake
//! local server (see `tests/`), never a real venue.

use std::sync::Arc;
use std::time::Duration;

use futures::{SinkExt, StreamExt};
use senken_marketdata::InstrumentId;
use senken_subscription::{ConnectionError, SubscriptionPool, VenueConnection};
use senken_venue::LimitGroup;
use tokio::sync::{Mutex, mpsc, watch};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use senken_subscription::{LiveUpdate, VenueProtocol};

use crate::proxy::HttpProxy;

/// Cost charged against a venue's [`LimitGroup`] for one WebSocket dial.
/// no venue's real connection weight is verified anywhere in
/// this project, so this is a uniform, conservative unit — the same
/// reasoning `senken_venue::INSTRUMENT_FETCH_COST` already applies to an
/// unverified catalog-fetch weight.
const WS_CONNECT_COST: u32 = 1;

/// The first backoff before a *reconnect* attempt (not the bounded initial
/// dial in [`crate::WsVenueConnector::connect`], which uses
/// [`senken_venue::RetryPolicy::INTERACTIVE`]'s own constant). Doubles from
/// here, capped at [`RECONNECT_BACKOFF_CAP`], then run through
/// [`senken_venue::full_jitter`]. Our own policy, not a venue fact — no
/// venue documents how quickly a dropped stream should be redialed.
const RECONNECT_FIRST_BACKOFF: Duration = Duration::from_millis(250);

/// The backoff between reconnect attempts never grows past this, so a
/// long-dead venue is still retried at a bounded interval rather than one
/// that grows for as long as the process keeps failing to reconnect.
const RECONNECT_BACKOFF_CAP: Duration = Duration::from_secs(30);

/// The backoff before reconnect attempt `attempt` (`1` for the first retry
/// after the initial connection drops), before jitter.
fn reconnect_backoff(attempt: u32) -> Duration {
    let doublings = attempt.saturating_sub(1).min(16); // avoid overflow on `2^n`
    RECONNECT_FIRST_BACKOFF
        .saturating_mul(1u32.checked_shl(doublings).unwrap_or(u32::MAX))
        .min(RECONNECT_BACKOFF_CAP)
}

/// One connected socket, before it is split into its independently-owned
/// halves.
type WsSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;
type WsSink = futures::stream::SplitSink<WsSocket, WsMessage>;
type WsStream = futures::stream::SplitStream<WsSocket>;

/// Counts every [`WsVenueConnection::dial_once`] attempt, across every
/// connection in the process — test-only instrumentation for
/// `reconnect_does_not_spin_between_failed_attempts`, compiled out of any
/// non-test build.
/// The proxy to tunnel `url` through, with the host and port to ask it for —
/// or `None` when this host should be dialled directly.
///
/// A URL this cannot parse dials directly rather than failing: the
/// WebSocket client is about to parse it too, and it gives a far better
/// error for a malformed URL than a proxy layer guessing at one would.
fn proxy_target(url: &str) -> Option<(HttpProxy, String, u16)> {
    let rest = url.split_once("://")?.1;
    let authority = rest.split(['/', '?']).next()?;
    let (host, port) = match authority.rsplit_once(':') {
        Some((host, port)) => (host.to_owned(), port.parse().ok()?),
        // `wss` is TLS and `ws` is not, so their default ports differ. The
        // explicit form above still has to work: a venue is free to serve
        // its stream on another port (OKX documents 8443 for its public
        // feed), and a proxy must be asked for the port the URL names.
        None => (
            authority.to_owned(),
            if url.starts_with("wss://") { 443 } else { 80 },
        ),
    };
    if host.is_empty() {
        return None;
    }
    HttpProxy::for_host(&host).map(|proxy| (proxy, host, port))
}

#[cfg(test)]
static DIAL_ATTEMPTS_FOR_TEST: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// A real [`VenueConnection`]: one live (or currently reconnecting)
/// WebSocket to a venue, per [`VenueProtocol`] `P`.
pub struct WsVenueConnection<P: VenueProtocol + ?Sized> {
    protocol: Arc<P>,
    group: LimitGroup,
    pool: SubscriptionPool,
    /// The current socket's outbound half, behind a lock so
    /// `subscribe`/`unsubscribe` (`&self`, called concurrently by the
    /// pool's serialised actor — never actually concurrent with each other,
    /// but this type has no other way to express interior mutability) can
    /// reach whichever socket is live right now. `None` while a reconnect
    /// is in progress.
    outbound: Mutex<Option<mpsc::UnboundedSender<WsMessage>>>,
    /// Signals the background read/reconnect task to stop, breaking the
    /// `Arc` cycle that task's own clone of `self` would otherwise hold
    /// forever.
    /// Flipped to `true` once, when this connection is retired.
    ///
    /// A `watch` channel rather than a `Notify`: `Notify::notify_waiters`
    /// only wakes tasks that are *already registered*, so a stop signalled
    /// while the owning task happened to be decoding a frame — rather than
    /// sitting in its `select!` — is simply lost, and the task runs on
    /// forever. `watch` holds the value, so a waiter that arrives after the
    /// signal still sees it.
    shutdown: watch::Sender<bool>,
}

impl<P: VenueProtocol + ?Sized> WsVenueConnection<P> {
    /// Dials `protocol.url()` once, gated by `group`'s shared budget
    /// (a venue's WS dial draws on the same budget as its REST traffic).
    async fn dial_once(
        protocol: &P,
        group: &LimitGroup,
    ) -> Result<(WsSink, WsStream), ConnectionError> {
        #[cfg(test)]
        DIAL_ATTEMPTS_FOR_TEST.fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        let _permit = group
            .acquire_for_connect(WS_CONNECT_COST)
            .await
            .map_err(|source| ConnectionError::new(source.to_string()))?;
        let url = protocol.endpoint().await?;
        let url = url.as_str();
        let (stream, _response) = match proxy_target(url) {
            // No proxy configured for this host: dial it directly, exactly
            // as before.
            None => tokio_tungstenite::connect_async(url)
                .await
                .map_err(|source| ConnectionError::new(source.to_string()))?,
            Some((proxy, host, port)) => {
                tracing::debug!(
                    venue = protocol.venue(),
                    %host,
                    "dialling the venue WebSocket through the configured proxy"
                );
                let tunnel = proxy.tunnel(&host, port).await?;
                // TLS, if the URL asks for it, is negotiated end-to-end
                // inside the tunnel — the proxy carries opaque bytes and
                // never sees the venue traffic.
                tokio_tungstenite::client_async_tls(url, tunnel)
                    .await
                    .map_err(|source| ConnectionError::new(source.to_string()))?
            }
        };
        Ok(stream.split())
    }

    /// Dials with a bounded number of attempts and jittered doubling
    /// backoff — used only for the very first connection
    /// ([`crate::WsVenueConnector::connect`]), which the pool expects to be
    /// able to fail outright (`PoolError::Connect`) rather than retry
    /// forever.
    async fn dial_with_bounded_retry(
        protocol: &P,
        group: &LimitGroup,
    ) -> Result<(WsSink, WsStream), ConnectionError> {
        let policy = senken_venue::RetryPolicy::INTERACTIVE;
        let mut backoff = policy.first_backoff;
        let mut last_error = None;
        for attempt in 1..=policy.max_attempts {
            match Self::dial_once(protocol, group).await {
                Ok(streams) => return Ok(streams),
                Err(error) => {
                    tracing::warn!(
                        venue = protocol.venue(),
                        attempt,
                        %error,
                        "initial connect attempt failed"
                    );
                    last_error = Some(error);
                    if attempt < policy.max_attempts {
                        tokio::time::sleep(senken_venue::full_jitter(backoff)).await;
                        backoff *= 2;
                    }
                }
            }
        }
        Err(last_error.unwrap_or_else(|| ConnectionError::new("no connect attempt was made")))
    }

    /// Builds a connection already holding its first live socket, and
    /// spawns the background task that owns every socket after this one.
    pub(crate) async fn connect(
        protocol: Arc<P>,
        group: LimitGroup,
        pool: SubscriptionPool,
    ) -> Result<Arc<Self>, ConnectionError> {
        let (sink, stream) = Self::dial_with_bounded_retry(&protocol, &group).await?;
        let (outbound_tx, outbound_rx) = mpsc::unbounded_channel();

        let connection = Arc::new(Self {
            protocol,
            group,
            pool,
            outbound: Mutex::new(Some(outbound_tx)),
            shutdown: watch::channel(false).0,
        });

        tokio::spawn(Arc::clone(&connection).run(sink, stream, outbound_rx));
        Ok(connection)
    }

    /// Owns every socket this connection ever has, for its entire life:
    /// forwards outbound frames, decodes and publishes inbound ones, and —
    /// once the live socket ends for any reason — redials with jittered
    /// backoff and calls [`SubscriptionPool::reconnected`] once it is back
    /// up ("the pool is the authority on what to
    /// re-subscribe").
    async fn run(
        self: Arc<Self>,
        mut sink: WsSink,
        mut stream: WsStream,
        mut outbound_rx: mpsc::UnboundedReceiver<WsMessage>,
    ) {
        // The very first socket was handed in by `connect`, so the pool
        // already subscribed onto it directly (via `VenueConnection::subscribe`)
        // rather than through a `reconnected` replay — only a socket born
        // from this loop's own redial needs that replay.
        let mut first_socket = true;

        loop {
            if !first_socket {
                let replayed = self.pool.reconnected(&(self.clone() as _)).await;
                if let Err(error) = replayed {
                    tracing::warn!(
                        venue = self.protocol.venue(),
                        %error,
                        "reconnect replay was rejected by the pool"
                    );
                }
            }
            first_socket = false;

            // A venue that wants an unprompted ping gets one on its own
            // interval; one that does not is parked on a timer that never
            // fires, so the `select!` arm below costs it nothing.
            let keepalive = self.protocol.keepalive();
            let mut ticker = keepalive.as_ref().map(|(every, _)| {
                // `interval_at`, not `interval`: the latter fires its first
                // tick immediately, sending a keep-alive to a socket that
                // has not even been subscribed on yet.
                let mut ticker =
                    tokio::time::interval_at(tokio::time::Instant::now() + *every, *every);
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                ticker
            });

            loop {
                tokio::select! {
                    biased;
                    () = wait_for_shutdown(&self.shutdown) => return,
                    () = tick(ticker.as_mut()) => {
                        let Some((_, frame)) = keepalive.as_ref() else { continue };
                        if sink.send(WsMessage::text(frame.clone())).await.is_err() {
                            break;
                        }
                    }
                    outgoing = outbound_rx.recv() => {
                        let Some(outgoing) = outgoing else { return };
                        if sink.send(outgoing).await.is_err() {
                            break;
                        }
                    }
                    incoming = stream.next() => {
                        let text = match incoming {
                            Some(Ok(WsMessage::Text(text))) => Some(text.to_string()),
                            // Not "nothing to act on": three venues in this
                            // project send every frame as binary — HTX and
                            // BingX gzip theirs, Upbit sends plain UTF-8
                            // JSON in a binary frame. Dropping these
                            // silently delivers no data at all from them.
                            Some(Ok(WsMessage::Binary(bytes))) => self.protocol.decode_binary(&bytes),
                            Some(Ok(WsMessage::Close(_))) => {
                                // A graceful close is still a disconnect:
                                // reconnect rather than keep waiting on a
                                // stream that has told us it is ending.
                                break;
                            }
                            // Ping/Pong/Frame: the transport answers
                            // WebSocket control frames itself.
                            Some(Ok(_control)) => None,
                            Some(Err(error)) => {
                                tracing::warn!(venue = self.protocol.venue(), %error, "read error; reconnecting");
                                break;
                            }
                            None => break,
                        };
                        let Some(text) = text else { continue };
                        for (instrument, update) in self.protocol.parse_message(&text) {
                            match update {
                                LiveUpdate::Price(update) => self.pool.publish(instrument, update),
                                LiveUpdate::Quote(update) => self.pool.publish_quote(instrument, update),
                            }
                        }
                        // After publishing, not before: a venue heartbeat
                        // that also carried data must not lose the data if
                        // the socket dies as the answer goes out.
                        if let Some(reply) = self.protocol.reply_to(&text)
                            && sink.send(WsMessage::text(reply)).await.is_err() {
                            break;
                        }
                    }
                }
            }

            *self.outbound.lock().await = None;

            let Some((new_sink, new_stream)) =
                Self::redial_until_up(&self.protocol, &self.group, &self.shutdown).await
            else {
                return; // shutdown was signalled while redialling
            };
            let (outbound_tx, new_outbound_rx) = mpsc::unbounded_channel();
            *self.outbound.lock().await = Some(outbound_tx);
            sink = new_sink;
            stream = new_stream;
            outbound_rx = new_outbound_rx;
        }
    }

    /// Redials forever, with jittered doubling backoff capped at
    /// [`RECONNECT_BACKOFF_CAP`], until a socket comes up or `shutdown`
    /// fires. Unlike [`dial_with_bounded_retry`](Self::dial_with_bounded_retry),
    /// an already-established connection with live leases on it does not
    /// get to give up — there is no way to report "permanently
    /// disconnected" back through the pool's existing bookkeeping, so it
    /// keeps trying at a bounded interval instead.
    ///
    /// Takes `protocol`/`group`/`shutdown` explicitly rather than `&self` so
    /// this crate's tests can drive it directly against a protocol pointed
    /// at a fake, unreachable address without constructing a whole
    /// [`WsVenueConnection`] (which itself requires a *successful* initial
    /// connect).
    async fn redial_until_up(
        protocol: &P,
        group: &LimitGroup,
        shutdown: &watch::Sender<bool>,
    ) -> Option<(WsSink, WsStream)> {
        let mut attempt: u32 = 1;
        loop {
            match Self::dial_once(protocol, group).await {
                Ok(streams) => return Some(streams),
                Err(error) => {
                    tracing::warn!(
                        venue = protocol.venue(),
                        attempt,
                        %error,
                        "reconnect attempt failed"
                    );
                    let wait = senken_venue::full_jitter(reconnect_backoff(attempt));
                    tokio::select! {
                        () = wait_for_shutdown(shutdown) => return None,
                        () = tokio::time::sleep(wait) => {}
                    }
                    attempt = attempt.saturating_add(1);
                }
            }
        }
    }

    async fn send(&self, frame: String) -> Result<(), ConnectionError> {
        let guard = self.outbound.lock().await;
        let Some(sender) = guard.as_ref() else {
            return Err(ConnectionError::new(
                "not currently connected; a reconnect is in progress",
            ));
        };
        sender
            .send(WsMessage::text(frame))
            .map_err(|_| ConnectionError::new("connection's write task has already ended"))
    }
}

/// Awaits `ticker`'s next tick, or never returns when there is no ticker.
///
/// `select!` needs a future in every arm even when this protocol has no
/// keep-alive; `pending()` is the arm that is simply never ready.
async fn tick(ticker: Option<&mut tokio::time::Interval>) {
    match ticker {
        Some(ticker) => {
            ticker.tick().await;
        }
        None => std::future::pending().await,
    }
}

impl<P: VenueProtocol + ?Sized> Drop for WsVenueConnection<P> {
    fn drop(&mut self) {
        // A backstop for the whole pool going away at once. The ordinary
        // path is `VenueConnection::shutdown`, called by the pool when a
        // shard empties: the task owning this connection's socket holds an
        // `Arc` to it for as long as it runs, so nothing else releasing its
        // handle can bring the strong count to zero and reach this.
        self.shutdown.send_replace(true);
    }
}

/// Resolves once `shutdown` has been signalled, now or later.
///
/// Checks the current value before waiting, which is what makes this
/// race-free: a stop flagged before the caller got here is still seen.
async fn wait_for_shutdown(shutdown: &watch::Sender<bool>) {
    let mut stop = shutdown.subscribe();
    // `wait_for` inspects the value already in the channel first, so this
    // returns immediately for a connection that is already retired.
    let _ = stop.wait_for(|stopping| *stopping).await;
}

#[async_trait::async_trait]
impl<P: VenueProtocol + ?Sized> VenueConnection for WsVenueConnection<P> {
    async fn subscribe(&self, instrument: &InstrumentId) -> Result<(), ConnectionError> {
        let frame = self.protocol.subscribe_frame(instrument)?;
        self.send(frame).await
    }

    async fn shutdown(&self) {
        // Ends `run`'s loop wherever it currently is — waiting on the
        // socket, or sleeping between redial attempts — which drops the
        // `Arc<Self>` that task holds and lets this connection actually be
        // freed. Idempotent: signalling an already-retired connection just
        // rewrites the same value.
        self.shutdown.send_replace(true);
    }

    async fn unsubscribe(&self, instrument: &InstrumentId) -> Result<(), ConnectionError> {
        let frame = self.protocol.unsubscribe_frame(instrument)?;
        self.send(frame).await
    }
}

#[cfg(test)]
mod tests {
    use super::{DIAL_ATTEMPTS_FOR_TEST, WsVenueConnection, reconnect_backoff};
    use senken_marketdata::InstrumentId;
    use senken_subscription::ConnectionError;
    use senken_subscription::{LiveUpdate, VenueProtocol};
    use senken_venue::LimitGroup;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    /// Enough of a [`VenueProtocol`] to dial `url` — no test in this module
    /// ever reaches `subscribe_frame`/`parse_message`, since it exercises
    /// only the connect/reconnect engine, never a live socket's contents.
    struct FakeProtocol {
        url: String,
    }

    impl VenueProtocol for FakeProtocol {
        fn url(&self) -> &str {
            &self.url
        }

        fn venue(&self) -> &'static str {
            "fake"
        }

        fn subscribe_frame(&self, _: &InstrumentId) -> Result<String, ConnectionError> {
            unreachable!("not exercised by this module's tests")
        }

        fn unsubscribe_frame(&self, _: &InstrumentId) -> Result<String, ConnectionError> {
            unreachable!("not exercised by this module's tests")
        }

        fn parse_message(&self, _: &str) -> Vec<(InstrumentId, LiveUpdate)> {
            unreachable!("not exercised by this module's tests")
        }
    }

    #[tokio::test]
    async fn reconnect_does_not_spin_between_failed_attempts() {
        // Bind a port, then immediately drop the listener: nothing answers
        // there any more, so every dial fails fast with "connection
        // refused" — precisely the repeated-failure case a spinning
        // implementation would hammer thousands of times per second.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        drop(listener);

        let protocol = FakeProtocol {
            url: format!("ws://{addr}"),
        };
        let group = LimitGroup::new("reconnect-spin-test");
        let shutdown = tokio::sync::watch::channel(false).0;

        DIAL_ATTEMPTS_FOR_TEST.store(0, Ordering::SeqCst);
        let window = Duration::from_millis(700);
        let outcome = tokio::time::timeout(
            window,
            WsVenueConnection::<FakeProtocol>::redial_until_up(&protocol, &group, &shutdown),
        )
        .await;

        assert!(
            outcome.is_err(),
            "the port is refused forever in this test, so redialling must still be in progress"
        );
        let attempts = DIAL_ATTEMPTS_FOR_TEST.load(Ordering::SeqCst);
        assert!(
            attempts >= 1,
            "at least the first attempt must have happened"
        );
        assert!(
            attempts <= 8,
            "backing off between attempts must keep this well under a spin \
             loop's count for the same {window:?} window; saw {attempts} attempts"
        );
    }

    #[test]
    fn backoff_doubles_then_caps() {
        assert_eq!(reconnect_backoff(1), Duration::from_millis(250));
        assert_eq!(reconnect_backoff(2), Duration::from_millis(500));
        assert_eq!(reconnect_backoff(3), Duration::from_secs(1));
        assert_eq!(
            reconnect_backoff(20),
            super::RECONNECT_BACKOFF_CAP,
            "backoff must not grow without bound"
        );
    }

    #[test]
    fn jitter_actually_varies_the_wait() {
        // Reuses `senken_venue::full_jitter`, itself already tested in its
        // own crate — this only proves the two are wired together, not that
        // jitter itself works.
        let base = reconnect_backoff(4);
        let samples: std::collections::HashSet<Duration> =
            (0..50).map(|_| senken_venue::full_jitter(base)).collect();
        assert!(samples.len() > 1);
        assert!(samples.iter().all(|s| *s <= base));
    }
}
