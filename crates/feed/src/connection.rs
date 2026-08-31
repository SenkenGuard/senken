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
use tokio::sync::{Mutex, Notify, mpsc};
use tokio_tungstenite::tungstenite::Message as WsMessage;

use crate::protocol::VenueProtocol;

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
#[cfg(test)]
static DIAL_ATTEMPTS_FOR_TEST: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// A real [`VenueConnection`]: one live (or currently reconnecting)
/// WebSocket to a venue, per [`VenueProtocol`] `P`.
pub struct WsVenueConnection<P: VenueProtocol> {
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
    shutdown: Arc<Notify>,
}

impl<P: VenueProtocol> WsVenueConnection<P> {
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
        let (stream, _response) = tokio_tungstenite::connect_async(protocol.url())
            .await
            .map_err(|source| ConnectionError::new(source.to_string()))?;
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
            shutdown: Arc::new(Notify::new()),
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

            loop {
                tokio::select! {
                    biased;
                    () = self.shutdown.notified() => return,
                    outgoing = outbound_rx.recv() => {
                        let Some(outgoing) = outgoing else { return };
                        if sink.send(outgoing).await.is_err() {
                            break;
                        }
                    }
                    incoming = stream.next() => {
                        match incoming {
                            Some(Ok(WsMessage::Text(text))) => {
                                for (instrument, update) in self.protocol.parse_message(&text) {
                                    self.pool.publish(instrument, update);
                                }
                            }
                            Some(Ok(WsMessage::Close(_))) => {
                                // A graceful close is still a disconnect:
                                // reconnect rather than keep waiting on a
                                // stream that has told us it is ending.
                                break;
                            }
                            Some(Ok(_non_text)) => {
                                // Ping/Pong/Binary/Frame: nothing this
                                // protocol needs to act on. Application-level
                                // keep-alive (some venues expect a text
                                // "ping"/"pong" exchange rather than WS
                                // control frames) was never observed in this
                                // milestone's one live capture — a
                                // documented gap, not an invented behaviour.
                            }
                            Some(Err(error)) => {
                                tracing::warn!(venue = self.protocol.venue(), %error, "read error; reconnecting");
                                break;
                            }
                            None => break,
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
        shutdown: &Notify,
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
                        () = shutdown.notified() => return None,
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

impl<P: VenueProtocol> Drop for WsVenueConnection<P> {
    fn drop(&mut self) {
        self.shutdown.notify_waiters();
    }
}

#[async_trait::async_trait]
impl<P: VenueProtocol> VenueConnection for WsVenueConnection<P> {
    async fn subscribe(&self, instrument: &InstrumentId) -> Result<(), ConnectionError> {
        let frame = self.protocol.subscribe_frame(instrument)?;
        self.send(frame).await
    }

    async fn unsubscribe(&self, instrument: &InstrumentId) -> Result<(), ConnectionError> {
        let frame = self.protocol.unsubscribe_frame(instrument)?;
        self.send(frame).await
    }
}

#[cfg(test)]
mod tests {
    use super::{DIAL_ATTEMPTS_FOR_TEST, WsVenueConnection, reconnect_backoff};
    use crate::protocol::VenueProtocol;
    use senken_marketdata::InstrumentId;
    use senken_subscription::{ConnectionError, PriceUpdate};
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

        fn parse_message(&self, _: &str) -> Vec<(InstrumentId, PriceUpdate)> {
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
        let shutdown = tokio::sync::Notify::new();

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
