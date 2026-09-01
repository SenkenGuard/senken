//! [`SubscriptionPool`] — reference-counted, `Drop`-guarded leases on
//! `(source, symbol)`, sharded across a venue's per-connection stream cap.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use senken_marketdata::InstrumentId;
use tokio::sync::{mpsc, oneshot, watch};

use crate::connection::{ConnectionError, VenueConnection, VenueConnector};
use crate::price::PriceUpdate;
use crate::quote::QuoteUpdate;

/// How a leaseholder learns the latest price for its instrument: `None`
/// until the first update arrives for a freshly (re)subscribed instrument,
/// `Some` after. A [`watch`] channel, not [`tokio::sync::broadcast`] or an
/// mpsc queue: the contract is "the latest price", not "every
/// price that ever arrived" — a slow consumer should see the newest value
/// next time it looks, not fall behind and have to catch up through a
/// backlog of stale ones.
type PriceWatch = watch::Sender<Option<PriceUpdate>>;
type QuoteWatch = watch::Sender<Option<QuoteUpdate>>;

struct LeaseWatches {
    price: PriceWatch,
    quote: QuoteWatch,
}

/// No venue's real per-connection stream cap is verified anywhere in this
/// project (mirroring
/// `senken_venue`'s own unverified `DEFAULT_MAX_CONCURRENT`). This is a
/// deliberately conservative assumption for any [`SubscriptionPool`] built
/// with [`SubscriptionPool::new`] rather than [`SubscriptionPool::with_cap`]
///   — small enough that even a strict venue is unlikely to reject a
/// connection outright for carrying this many streams, and never treated as
/// fact anywhere else in this crate.
const DEFAULT_STREAM_CAP: usize = 50;

/// Why a [`SubscriptionPool`] operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PoolError {
    /// Every existing shard for this venue was at its cap, and opening a new
    /// one failed too.
    #[error("{venue}: could not open a new connection: {source}")]
    Connect {
        /// The venue this pool serves.
        venue: Box<str>,
        /// Why the connector failed.
        #[source]
        source: ConnectionError,
    },

    /// The venue rejected a subscribe call for one instrument.
    #[error("{venue}: could not subscribe {instrument}: {source}")]
    Subscribe {
        /// The venue this pool serves.
        venue: Box<str>,
        /// The instrument that could not be subscribed.
        instrument: InstrumentId,
        /// Why the connection rejected it.
        #[source]
        source: ConnectionError,
    },

    /// [`SubscriptionPool::reconnected`] was called with a connection this
    /// pool never opened — a caller bug (a connection from a different
    /// pool, or one this pool has since forgotten), not a venue failure.
    #[error("{venue}: reconnect reported for a connection this pool never opened")]
    UnknownConnection {
        /// The venue this pool serves.
        venue: Box<str>,
    },

    /// The pool's actor task is no longer running, so the request was never
    /// applied. Only reachable if that task panicked; a `SubscriptionPool`
    /// handle and the actor it talks to are created together and share the
    /// same lifetime otherwise.
    #[error("subscription pool's actor task is no longer running")]
    Closed,
}

/// One held claim on live data for `(source, symbol)`.
///
/// Obtained from [`SubscriptionPool::lease`]. Dropping it — not calling a
/// method on it, because there is deliberately no method to call — releases
/// the claim: on the *last* outstanding lease for this instrument, the pool
/// unsubscribes from the venue. See [`SubscriptionPool::lease`] for why this
/// is a `Drop` guard rather than a manual `unsubscribe`.
#[must_use = "dropping this immediately releases the lease"]
pub struct Lease {
    instrument: InstrumentId,
    // `None` once released, so `Drop` cannot send twice if a future version
    // of this type ever grows an explicit early-release method.
    release: Option<mpsc::UnboundedSender<Command>>,
    // The sender side of this instrument's `PriceWatch`, held only to mint
    // fresh receivers via `subscribe()` — never sent on from here. Every
    // leaseholder of the same instrument holds a clone of the same sender,
    // so they all read from the one channel the actor publishes updates
    // onto (see `Actor::publish`).
    updates: PriceWatch,
    quote_updates: QuoteWatch,
}

impl Lease {
    /// The instrument this lease claims.
    #[must_use]
    pub fn instrument(&self) -> &InstrumentId {
        &self.instrument
    }

    /// A receiver for this instrument's live price.
    ///
    /// the whole contract: anything holding a lease receives
    /// updates, with no further registration step. `None` until the first
    /// [`SubscriptionPool::publish`] for this instrument; `Some` after. Call
    /// this as many times as needed — each call mints an independent
    /// receiver starting from the channel's *current* value, so a consumer
    /// that subscribes late still sees the latest price immediately rather
    /// than waiting for the next tick.
    #[must_use]
    pub fn updates(&self) -> watch::Receiver<Option<PriceUpdate>> {
        self.updates.subscribe()
    }

    /// A receiver for this instrument's latest best bid and offer.
    #[must_use]
    pub fn quote_updates(&self) -> watch::Receiver<Option<QuoteUpdate>> {
        self.quote_updates.subscribe()
    }
}

impl std::fmt::Debug for Lease {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Lease")
            .field("instrument", &self.instrument)
            .finish_non_exhaustive()
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        // A non-async `Drop` cannot itself await the venue's unsubscribe
        // (exactly the part to design
        // deliberately). The send below is synchronous and infallible from
        // `Drop`'s point of view — it only ever enqueues onto the actor's
        // channel — so the actual async unsubscribe happens later, inside
        // the actor task, never inside this `drop`. A closed channel (the
        // actor task is gone) means there is nothing left to release
        // against, so the error is intentionally discarded.
        if let Some(release) = self.release.take() {
            let _ = release.send(Command::Release {
                instrument: self.instrument.clone(),
            });
        }
    }
}

/// The message an actor task processes one at a time, so lease/release/
/// reconnect never interleave and no lock is needed over the pool's state.
enum Command {
    Lease {
        instrument: InstrumentId,
        respond: oneshot::Sender<Result<LeaseWatches, PoolError>>,
    },
    Release {
        instrument: InstrumentId,
    },
    Reconnected {
        connection: Arc<dyn VenueConnection>,
        respond: oneshot::Sender<Result<(), PoolError>>,
    },
    /// A price update for one instrument, from whatever [`VenueConnection`]
    /// decoded it off the wire. Fire-and-forget, unlike every other command
    /// here: a tick that arrives for an instrument nobody currently leases
    /// (a race at the tail end of an unsubscribe) is simply dropped, not an
    /// error — see [`SubscriptionPool::publish`].
    Publish {
        instrument: InstrumentId,
        update: PriceUpdate,
    },
    PublishQuote {
        instrument: InstrumentId,
        update: QuoteUpdate,
    },
    /// Answered only once every command sent before it has been applied —
    /// see [`SubscriptionPool::flush`].
    Flush {
        respond: oneshot::Sender<()>,
    },
}

/// How many leaseholders are currently holding `(source, symbol)`, which
/// shard carries its venue subscription, and the channel every one of those
/// leaseholders' [`Lease::updates`] receivers ultimately reads from.
struct LeaseRecord {
    count: usize,
    shard: usize,
    updates: PriceWatch,
    quote_updates: QuoteWatch,
}

/// One connection to the venue, and the instruments currently subscribed on
/// it.
struct Shard {
    connection: Arc<dyn VenueConnection>,
    symbols: HashSet<InstrumentId>,
}

/// Owns every mutable field of a [`SubscriptionPool`] and is the only thing
/// that ever calls [`VenueConnection::subscribe`]/`unsubscribe` or
/// [`VenueConnector::connect`]. Reachable only through the actor task
/// [`SubscriptionPool::new`] spawns, so its methods never need a lock.
struct Actor {
    venue: Box<str>,
    connector: Box<dyn VenueConnector>,
    cap_per_connection: usize,
    shards: Vec<Shard>,
    leases: HashMap<InstrumentId, LeaseRecord>,
}

impl Actor {
    async fn run(mut self, mut commands: mpsc::UnboundedReceiver<Command>) {
        while let Some(command) = commands.recv().await {
            match command {
                Command::Lease {
                    instrument,
                    respond,
                } => {
                    let result = self.lease(&instrument).await;
                    // The caller may have stopped waiting (e.g. its `lease`
                    // future was cancelled); that is not this task's
                    // problem, so a failed send is silently ignored rather
                    // than logged as an error.
                    let _ = respond.send(result);
                }
                Command::Release { instrument } => self.release(&instrument).await,
                Command::Reconnected {
                    connection,
                    respond,
                } => {
                    let result = self.reconnected(&connection).await;
                    let _ = respond.send(result);
                }
                Command::Publish { instrument, update } => self.publish(&instrument, update),
                Command::PublishQuote { instrument, update } => {
                    self.publish_quote(&instrument, update);
                }
                Command::Flush { respond } => {
                    let _ = respond.send(());
                }
            }
        }
    }

    async fn lease(&mut self, instrument: &InstrumentId) -> Result<LeaseWatches, PoolError> {
        if let Some(record) = self.leases.get_mut(instrument) {
            record.count += 1;
            return Ok(LeaseWatches {
                price: record.updates.clone(),
                quote: record.quote_updates.clone(),
            });
        }

        let shard_index = self.shard_with_room().await?;
        self.shards[shard_index]
            .connection
            .subscribe(instrument)
            .await
            .map_err(|source| PoolError::Subscribe {
                venue: self.venue.clone(),
                instrument: instrument.clone(),
                source,
            })?;

        self.shards[shard_index].symbols.insert(instrument.clone());
        let (updates, _receiver) = watch::channel(None);
        let (quote_updates, _quote_receiver) = watch::channel(None);
        self.leases.insert(
            instrument.clone(),
            LeaseRecord {
                count: 1,
                shard: shard_index,
                updates: updates.clone(),
                quote_updates: quote_updates.clone(),
            },
        );
        Ok(LeaseWatches {
            price: updates,
            quote: quote_updates,
        })
    }

    /// Routes one decoded price to every current leaseholder of
    /// `instrument`, via that instrument's [`PriceWatch`].
    ///
    /// An instrument with no recorded lease is not an error worth logging
    /// loudly: a [`VenueConnection`] and this actor's own bookkeeping are
    /// two different tasks, so a tick can legitimately arrive for an
    /// instrument whose last lease was released moments earlier, before the
    /// unsubscribe this actor already issued has silenced the venue's own
    /// stream.
    fn publish(&self, instrument: &InstrumentId, update: PriceUpdate) {
        let Some(record) = self.leases.get(instrument) else {
            tracing::debug!(
                venue = %self.venue,
                %instrument,
                "dropping a price update for an instrument with no current lease"
            );
            return;
        };
        // `send`, not `send_replace`, would silently drop this value
        // instead of recording it whenever zero receivers are currently
        // alive — every `Lease` for this instrument momentarily has none
        // subscribed, or none has called `updates()` yet. `send_replace`
        // updates the watched value unconditionally, which is what "a
        // consumer that subscribes late still sees the latest price"
        // (`Lease::updates`'s own contract) requires.
        let _ = record.updates.send_replace(Some(update));
    }

    fn publish_quote(&self, instrument: &InstrumentId, update: QuoteUpdate) {
        let Some(record) = self.leases.get(instrument) else {
            return;
        };
        let _ = record.quote_updates.send_replace(Some(update));
    }

    async fn release(&mut self, instrument: &InstrumentId) {
        let Some(record) = self.leases.get_mut(instrument) else {
            // Only `Lease::drop` sends this, exactly once per successful
            // `lease` call it made — this branch is unreachable in
            // practice. Ignoring it (rather than panicking the whole actor,
            // which would silently stop every other leaseholder's traffic
            // too) is the safe default if that invariant is ever broken.
            tracing::warn!(
                venue = %self.venue,
                %instrument,
                "release for an instrument with no recorded lease"
            );
            return;
        };

        record.count -= 1;
        if record.count > 0 {
            return;
        }
        let shard_index = self.leases.remove(instrument).map(|record| record.shard);
        let Some(shard_index) = shard_index else {
            return;
        };
        let shard = &mut self.shards[shard_index];
        if let Err(error) = shard.connection.unsubscribe(instrument).await {
            // The venue rejecting an unsubscribe must not leave this
            // instrument's bookkeeping — and therefore its slot against the
            // shard's cap — pinned forever on a leaseholder that has
            // already dropped its guard and moved on.
            tracing::warn!(
                venue = %self.venue,
                %instrument,
                %error,
                "venue rejected unsubscribe; releasing bookkeeping anyway"
            );
        }
        shard.symbols.remove(instrument);
    }

    /// The index of a shard with room for one more instrument, opening a new
    /// connection if every existing shard is already at
    /// `cap_per_connection`.
    async fn shard_with_room(&mut self) -> Result<usize, PoolError> {
        if let Some(index) = self
            .shards
            .iter()
            .position(|shard| shard.symbols.len() < self.cap_per_connection)
        {
            return Ok(index);
        }

        let connection = self
            .connector
            .connect(&self.venue)
            .await
            .map_err(|source| PoolError::Connect {
                venue: self.venue.clone(),
                source,
            })?;
        self.shards.push(Shard {
            connection,
            symbols: HashSet::new(),
        });
        Ok(self.shards.len() - 1)
    }

    /// Replays a subscribe for every instrument currently leased on
    /// `connection`'s shard — "the pool is the authority on what to
    /// re-subscribe".
    async fn reconnected(&self, connection: &Arc<dyn VenueConnection>) -> Result<(), PoolError> {
        let Some(shard) = self
            .shards
            .iter()
            .find(|shard| Arc::ptr_eq(&shard.connection, connection))
        else {
            return Err(PoolError::UnknownConnection {
                venue: self.venue.clone(),
            });
        };

        for instrument in &shard.symbols {
            connection
                .subscribe(instrument)
                .await
                .map_err(|source| PoolError::Subscribe {
                    venue: self.venue.clone(),
                    instrument: instrument.clone(),
                    source,
                })?;
        }
        Ok(())
    }
}

/// A reference-counted registry of live-data leases for one venue.
///
/// Nothing polls. A chart pane, a watchlist row, an alert or
/// an open position all call [`lease`](Self::lease) the same way; the pool
/// subscribes to the venue on a `(source, symbol)`'s first lease and
/// unsubscribes on its last, and knows nothing about any of them beyond
/// that.
///
/// Cheap to clone: every clone shares the same actor task and state,
/// exactly like `senken_venue::LimitGroup` — a plugin that registers
/// several sources for one venue shares one `SubscriptionPool` across them,
/// so a `binance-spot` lease and a `binance-usdm` lease drawing on the same
/// physical venue connections shard together rather than each getting their
/// own private cap.
#[derive(Clone)]
pub struct SubscriptionPool {
    venue: Box<str>,
    commands: mpsc::UnboundedSender<Command>,
}

impl std::fmt::Debug for SubscriptionPool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubscriptionPool")
            .field("venue", &self.venue)
            .finish_non_exhaustive()
    }
}

impl SubscriptionPool {
    /// A pool for `venue`, opening connections through `connector`, with the
    /// conservative default per-connection stream cap (`DEFAULT_STREAM_CAP`).
    ///
    /// Spawns the pool's actor task immediately with [`tokio::spawn`], so
    /// this must be called from inside a Tokio runtime.
    #[must_use]
    pub fn new(venue: impl Into<Box<str>>, connector: impl VenueConnector) -> Self {
        Self::with_cap(venue, connector, DEFAULT_STREAM_CAP)
    }

    /// As [`new`](Self::new), but with an explicit per-connection stream cap
    /// rather than the conservative default — the "configurable
    /// per venue".
    ///
    /// `cap_per_connection` is clamped to at least one: a cap of zero could
    /// never admit a single subscription and would only ever open
    /// connections without using any of them.
    #[must_use]
    pub fn with_cap(
        venue: impl Into<Box<str>>,
        connector: impl VenueConnector,
        cap_per_connection: usize,
    ) -> Self {
        let venue = venue.into();
        let (commands, receiver) = mpsc::unbounded_channel();
        let actor = Actor {
            venue: venue.clone(),
            connector: Box::new(connector),
            cap_per_connection: cap_per_connection.max(1),
            shards: Vec::new(),
            leases: HashMap::new(),
        };
        tokio::spawn(actor.run(receiver));
        Self { venue, commands }
    }

    /// The venue this pool serves.
    #[must_use]
    pub fn venue(&self) -> &str {
        &self.venue
    }

    /// Claims a lease on `instrument`.
    ///
    /// If another leaseholder already holds `instrument`, this only
    /// increments the pool's reference count — no venue call is made. If
    /// this is the first lease, the pool subscribes on a shard with room
    /// under the venue's configured cap (opening a new connection if none
    /// has room) before returning it, so a successful `lease` always means
    /// the subscription is live, not merely requested.
    ///
    /// Dropping the returned [`Lease`] is the only way to release it —
    /// there is deliberately no `unsubscribe` method. A manual-release API
    /// relies on every caller remembering to call it; a pane that closes
    /// without releasing would then leak a subscription silently, until a
    /// venue's connection cap is hit and unrelated panes start failing to
    /// open. Making release a `Drop` effect makes that leak unrepresentable
    /// instead of merely discouraged.
    ///
    /// # Errors
    /// [`PoolError::Connect`] if a new connection was needed and could not
    /// be opened, [`PoolError::Subscribe`] if the venue rejected the
    /// subscribe call, or [`PoolError::Closed`] if the pool's actor task is
    /// no longer running.
    pub async fn lease(&self, instrument: InstrumentId) -> Result<Lease, PoolError> {
        let (respond, response) = oneshot::channel();
        self.commands
            .send(Command::Lease {
                instrument: instrument.clone(),
                respond,
            })
            .map_err(|_| PoolError::Closed)?;
        let watches = response.await.map_err(|_| PoolError::Closed)??;
        Ok(Lease {
            instrument,
            release: Some(self.commands.clone()),
            updates: watches.price,
            quote_updates: watches.quote,
        })
    }

    /// Delivers `update` to every current leaseholder of `instrument`.
    ///
    /// Called by a real [`VenueConnection`] every time it decodes a price
    /// off the wire — not part of the [`VenueConnection`] trait itself,
    /// because publishing is a data-plane concern separate from the
    /// subscribe/unsubscribe control plane that trait exists for, and a
    /// connection already holds a [`SubscriptionPool`] handle of its own
    /// (this type is cheap to clone) rather than needing a new capability
    /// threaded through the trait.
    ///
    /// Fire-and-forget: there is deliberately no way to await this
    /// finishing (contrast [`flush`](Self::flush), which exists precisely so
    /// a caller *can* wait when it needs to) and no error is returned for an
    /// instrument nobody currently leases — a tick racing the tail end of an
    /// unsubscribe is an ordinary, harmless occurrence, not a caller
    /// mistake. If the pool's actor task is no longer running, the update is
    /// silently dropped, the same way a [`Lease`] being dropped after the
    /// actor is gone has nothing left to release against.
    pub fn publish(&self, instrument: InstrumentId, update: PriceUpdate) {
        let _ = self.commands.send(Command::Publish { instrument, update });
    }

    /// Delivers a top-of-book quote to every current leaseholder.
    pub fn publish_quote(&self, instrument: InstrumentId, update: QuoteUpdate) {
        let _ = self
            .commands
            .send(Command::PublishQuote { instrument, update });
    }

    /// Tells the pool that `connection` — a shard it previously opened
    /// through its [`VenueConnector`] — has reconnected, so every instrument
    /// currently leased on that shard should be subscribed again.
    ///
    /// The pool never dials a socket itself (see [`VenueConnection`]'s
    /// docs); this is how the connection's own owner, once its reconnect
    /// (with whatever backoff it uses) succeeds, tells the
    /// pool to replay what that shard is supposed to carry. Only the
    /// instruments leased on `connection`'s own shard are replayed — a
    /// venue with several shards does not have the others' subscriptions
    /// disturbed by one shard reconnecting.
    ///
    /// # Errors
    /// [`PoolError::UnknownConnection`] if `connection` is not a shard this
    /// pool opened, [`PoolError::Subscribe`] if the venue rejected a
    /// replayed subscribe, or [`PoolError::Closed`] if the pool's actor task
    /// is no longer running.
    pub async fn reconnected(
        &self,
        connection: &Arc<dyn VenueConnection>,
    ) -> Result<(), PoolError> {
        let (respond, response) = oneshot::channel();
        self.commands
            .send(Command::Reconnected {
                connection: Arc::clone(connection),
                respond,
            })
            .map_err(|_| PoolError::Closed)?;
        response.await.map_err(|_| PoolError::Closed)?
    }

    /// Waits until every lease, release and reconnect sent to this pool
    /// before this call was made has finished being applied.
    ///
    /// Not needed for correctness — a dropped lease's release is always
    /// eventually applied, in the order it was dropped, whether or not
    /// anything ever calls this. It exists for deterministic shutdown and
    /// for tests that must observe a release's effect (a call to
    /// [`VenueConnection::unsubscribe`]) before asserting on it, since the
    /// effect happens asynchronously on the actor task rather than inside
    /// [`Lease`]'s `Drop`.
    pub async fn flush(&self) {
        let (respond, response) = oneshot::channel();
        if self.commands.send(Command::Flush { respond }).is_ok() {
            let _ = response.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{PoolError, PriceUpdate, SubscriptionPool};
    use crate::connection::{ConnectionError, VenueConnection, VenueConnector};
    use async_trait::async_trait;
    use senken_marketdata::InstrumentId;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Records every subscribe/unsubscribe call it receives, and reports how
    /// many times it was opened — a venue this crate never has to talk to
    /// for real's own constraint.
    #[derive(Default)]
    struct FakeConnection {
        subscribed: Mutex<Vec<InstrumentId>>,
        unsubscribed: Mutex<Vec<InstrumentId>>,
    }

    #[async_trait]
    impl VenueConnection for FakeConnection {
        async fn subscribe(&self, instrument: &InstrumentId) -> Result<(), ConnectionError> {
            self.subscribed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(instrument.clone());
            Ok(())
        }

        async fn unsubscribe(&self, instrument: &InstrumentId) -> Result<(), ConnectionError> {
            self.unsubscribed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(instrument.clone());
            Ok(())
        }
    }

    /// Hands out a fresh [`FakeConnection`] per `connect` call and keeps
    /// every one of them reachable, so a test can assert on a specific
    /// shard's call log or pass one back to [`SubscriptionPool::reconnected`].
    #[derive(Default)]
    struct FakeConnector {
        opened: Mutex<Vec<Arc<FakeConnection>>>,
        connects: AtomicUsize,
    }

    impl FakeConnector {
        fn connect_count(&self) -> usize {
            self.connects.load(Ordering::SeqCst)
        }

        fn opened_shards(&self) -> Vec<Arc<FakeConnection>> {
            self.opened
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    #[async_trait]
    impl VenueConnector for FakeConnector {
        async fn connect(&self, _venue: &str) -> Result<Arc<dyn VenueConnection>, ConnectionError> {
            self.connects.fetch_add(1, Ordering::SeqCst);
            let connection = Arc::new(FakeConnection::default());
            self.opened
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Arc::clone(&connection));
            Ok(connection)
        }
    }

    fn instrument(symbol: &str) -> InstrumentId {
        InstrumentId::new("fake-venue", symbol).unwrap()
    }

    #[tokio::test]
    async fn two_leaseholders_on_the_same_instrument_produce_one_subscription() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));

        let first = pool.lease(instrument("BTCUSDT")).await.unwrap();
        let second = pool.lease(instrument("BTCUSDT")).await.unwrap();

        assert_eq!(connector.connect_count(), 1);
        let shards = connector.opened_shards();
        assert_eq!(shards.len(), 1);
        assert_eq!(
            shards[0]
                .subscribed
                .lock()
                .unwrap()
                .iter()
                .filter(|i| **i == instrument("BTCUSDT"))
                .count(),
            1,
            "the venue must see exactly one subscribe for two leaseholders"
        );

        drop(first);
        drop(second);
    }

    #[tokio::test]
    async fn dropping_the_last_lease_unsubscribes_but_a_non_last_one_does_not() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));

        let first = pool.lease(instrument("ETHUSDT")).await.unwrap();
        let second = pool.lease(instrument("ETHUSDT")).await.unwrap();

        drop(first);
        pool.flush().await;
        let shard = &connector.opened_shards()[0];
        assert!(
            shard.unsubscribed.lock().unwrap().is_empty(),
            "a non-last lease must not unsubscribe"
        );

        drop(second);
        pool.flush().await;
        assert_eq!(
            shard.unsubscribed.lock().unwrap().as_slice(),
            &[instrument("ETHUSDT")],
            "the last lease must unsubscribe"
        );
    }

    #[tokio::test]
    async fn a_dropped_guard_cannot_leak_a_subscription() {
        // The leak scenario this guards: a pane (or anything else
        // holding a lease) closes without calling any explicit cleanup —
        // which is not even possible here, since `Lease` exposes no such
        // method. Its guard is simply dropped when its scope ends, exactly
        // as a closed pane's would be.
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));

        {
            let _lease = pool.lease(instrument("SOLUSDT")).await.unwrap();
            // Scope ends here with no explicit release.
        }

        pool.flush().await;
        let shard = &connector.opened_shards()[0];
        assert_eq!(
            shard.unsubscribed.lock().unwrap().as_slice(),
            &[instrument("SOLUSDT")],
            "the scope ending must have released the lease with no help from the caller"
        );

        // Proof the slot is actually free, not just that unsubscribe was
        // called: leasing the same instrument again must reuse the shard
        // rather than the pool believing it is still occupied.
        let _lease = pool.lease(instrument("SOLUSDT")).await.unwrap();
        assert_eq!(
            connector.connect_count(),
            1,
            "a freed slot must be reusable, proving nothing was left dangling"
        );
    }

    #[tokio::test]
    async fn reaching_the_configured_cap_shards_to_another_connection() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::with_cap("fake-venue", Arc::clone(&connector), 1);

        let first = pool.lease(instrument("AAA")).await.unwrap();
        let second = pool.lease(instrument("BBB")).await.unwrap();

        assert_eq!(
            connector.connect_count(),
            2,
            "a second instrument over a cap of one must open a second connection rather than fail"
        );
        let shards = connector.opened_shards();
        assert_eq!(
            shards[0].subscribed.lock().unwrap().as_slice(),
            &[instrument("AAA")]
        );
        assert_eq!(
            shards[1].subscribed.lock().unwrap().as_slice(),
            &[instrument("BBB")]
        );

        drop(first);
        drop(second);
    }

    #[tokio::test]
    async fn reconnect_resubscribes_exactly_what_is_currently_leased() {
        let connector = Arc::new(FakeConnector::default());
        // A cap of one keeps each instrument on its own shard, so the test
        // can assert that reconnecting one shard never touches the other.
        let pool = SubscriptionPool::with_cap("fake-venue", Arc::clone(&connector), 1);

        let first = pool.lease(instrument("AAA")).await.unwrap();
        let second = pool.lease(instrument("BBB")).await.unwrap();
        let shards = connector.opened_shards();
        let (shard_a, shard_b): (Arc<dyn VenueConnection>, Arc<dyn VenueConnection>) =
            (Arc::clone(&shards[0]) as _, Arc::clone(&shards[1]) as _);

        pool.reconnected(&shard_a).await.unwrap();

        assert_eq!(
            shards[0].subscribed.lock().unwrap().as_slice(),
            &[instrument("AAA"), instrument("AAA")],
            "reconnecting shard A must replay exactly its own one leased instrument"
        );
        assert_eq!(
            shards[1].subscribed.lock().unwrap().as_slice(),
            &[instrument("BBB")],
            "shard B must be untouched by shard A's reconnect"
        );

        drop(shard_b);
        drop(first);
        drop(second);
    }

    #[tokio::test]
    async fn reconnecting_an_unknown_connection_is_reported_not_silently_ignored() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));
        let stray: Arc<dyn VenueConnection> = Arc::new(FakeConnection::default());

        let error = pool.reconnected(&stray).await.unwrap_err();
        assert!(matches!(error, PoolError::UnknownConnection { .. }));
    }

    #[tokio::test]
    async fn a_cap_of_zero_is_clamped_so_a_lease_can_still_be_admitted() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::with_cap("fake-venue", Arc::clone(&connector), 0);

        let lease = pool.lease(instrument("ZZZ")).await;
        assert!(
            lease.is_ok(),
            "a misconfigured cap of zero must not make every lease fail"
        );
    }

    fn tick(price: i64) -> PriceUpdate {
        PriceUpdate {
            ts: senken_core::UnixNanos::from_millis(1_788_000_000_000).unwrap(),
            price,
            price_scale: 2,
            qty: senken_series::Volume::Real(0),
            qty_scale: 0,
        }
    }

    #[tokio::test]
    async fn a_lease_receives_updates_for_its_instrument_and_not_for_others() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));

        let btc = pool.lease(instrument("BTCUSDT")).await.unwrap();
        let eth = pool.lease(instrument("ETHUSDT")).await.unwrap();
        let mut btc_updates = btc.updates();
        let mut eth_updates = eth.updates();

        pool.publish(instrument("BTCUSDT"), tick(7_814_600));
        btc_updates.changed().await.unwrap();
        assert_eq!(*btc_updates.borrow(), Some(tick(7_814_600)));
        assert_eq!(
            *eth_updates.borrow(),
            None,
            "a price published for BTCUSDT must never reach ETHUSDT's leaseholder"
        );

        pool.publish(instrument("ETHUSDT"), tick(350_000));
        eth_updates.changed().await.unwrap();
        assert_eq!(*eth_updates.borrow(), Some(tick(350_000)));
        assert_eq!(
            *btc_updates.borrow(),
            Some(tick(7_814_600)),
            "ETHUSDT's price must never overwrite BTCUSDT's leaseholder's view"
        );

        drop(btc);
        drop(eth);
    }

    #[tokio::test]
    async fn two_leaseholders_on_one_instrument_both_see_the_same_updates() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));

        let first = pool.lease(instrument("SOLUSDT")).await.unwrap();
        let second = pool.lease(instrument("SOLUSDT")).await.unwrap();
        let mut first_updates = first.updates();
        let mut second_updates = second.updates();

        pool.publish(instrument("SOLUSDT"), tick(20_000));
        first_updates.changed().await.unwrap();
        second_updates.changed().await.unwrap();
        assert_eq!(*first_updates.borrow(), Some(tick(20_000)));
        assert_eq!(*second_updates.borrow(), Some(tick(20_000)));

        drop(first);
        drop(second);
    }

    #[tokio::test]
    async fn subscribing_late_still_sees_the_latest_price_immediately() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));

        let lease = pool.lease(instrument("ADAUSDT")).await.unwrap();
        pool.publish(instrument("ADAUSDT"), tick(50));
        // No `changed().await` here — `publish` is fire-and-forget, so wait
        // for the actor to have actually applied it before asserting.
        pool.flush().await;

        // A second call to `updates()` after the price already changed must
        // not have to wait for a further tick to see it.
        let late_updates = lease.updates();
        assert_eq!(*late_updates.borrow(), Some(tick(50)));

        drop(lease);
    }

    #[tokio::test]
    async fn a_price_for_an_instrument_with_no_lease_is_dropped_not_panicked() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));

        // Nothing leases "GHOST" at all — this must be a harmless no-op.
        pool.publish(instrument("GHOST"), tick(1));
        pool.flush().await;
    }
}
