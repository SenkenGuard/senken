//! The boundary between [`crate::SubscriptionPool`] and a real venue socket.
//!
//! Nothing in this crate opens a WebSocket. It needs only the pool's
//! *contract* with whatever eventually implements one, so that contract is
//! proven here against a fake: no test in this crate sends a request to any
//! venue.

use std::sync::Arc;

use async_trait::async_trait;
use senken_marketdata::InstrumentId;

/// Why a venue connection could not do what was asked of it.
///
/// Deliberately just a message: the concrete connection a real WS client
/// provides will have its own rich error type (socket, protocol,
/// venue-rejection...), and this crate has no way to know its shape today.
/// `ConnectionError::new` is how that error gets translated into something
/// this crate's [`crate::PoolError`] can carry and display.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct ConnectionError {
    message: String,
}

impl ConnectionError {
    /// Wraps `message` as a connection failure.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// One logical connection slot to a venue — a shard, in the pool's terms.
///
/// A real implementation owns one WebSocket to the venue, tracks its
/// own reconnect/backoff, and calls
/// [`SubscriptionPool::reconnected`](crate::SubscriptionPool::reconnected)
/// with its own `Arc<dyn VenueConnection>` once a dropped socket is back up,
/// so the pool can replay the subscribes that connection is supposed to
/// carry. The pool never dials or redials a socket itself — it only ever
/// asks a connection to subscribe or unsubscribe one instrument.
#[async_trait]
pub trait VenueConnection: Send + Sync + 'static {
    /// Subscribes to live updates for `instrument` on this connection.
    ///
    /// # Errors
    /// Whatever prevented the venue from accepting the subscription.
    async fn subscribe(&self, instrument: &InstrumentId) -> Result<(), ConnectionError>;

    /// Unsubscribes `instrument` from this connection.
    ///
    /// # Errors
    /// Whatever prevented the venue from accepting the unsubscription. The
    /// pool logs this rather than propagating it (see
    /// [`crate::SubscriptionPool::lease`]'s docs) — bookkeeping must not stay
    /// pinned on a leaseholder that has already dropped its guard.
    async fn unsubscribe(&self, instrument: &InstrumentId) -> Result<(), ConnectionError>;

    /// Closes this connection for good: nothing will be subscribed on it
    /// again, and any background task owning its socket must stop.
    ///
    /// Called by the pool when the last instrument on a connection's shard
    /// is released. Without it a venue socket outlives everyone watching
    /// it — an implementation that owns its socket from a spawned task
    /// typically holds a strong reference to itself for as long as that
    /// task runs, so nothing else dropping its handle can ever end it. That
    /// is not hypothetical: it is exactly what this method was added to
    /// fix.
    ///
    /// Must be safe to call more than once, and must not fail: a caller
    /// retiring a connection has already stopped using it and has nothing
    /// to do with an error.
    async fn shutdown(&self);
}

/// Opens new [`VenueConnection`]s for one venue, on demand.
///
/// The pool calls [`connect`](Self::connect) itself only when every existing
/// shard is at its configured cap (the "shard across connections
/// when it is reached") — never speculatively, and never in response to a
/// venue error, per the same section's instruction that the cap comes from
/// configuration rather than from a failure raised while a user is opening
/// a pane.
#[async_trait]
pub trait VenueConnector: Send + Sync + 'static {
    /// Opens one new connection to `venue`.
    ///
    /// # Errors
    /// Whatever prevented the connection from being established.
    async fn connect(&self, venue: &str) -> Result<Arc<dyn VenueConnection>, ConnectionError>;
}

// Lets a pool be built from a shared `Arc<impl VenueConnector>` — the shape
// a real venue's connector will already be in, since it must also be handed
// to whatever manages that venue's actual sockets — without needing its own
// wrapper type, and lets a test keep its own handle to a connector it has
// moved into a pool (this crate's tests do exactly that, to assert on calls
// the connector recorded).
#[async_trait]
impl<T: VenueConnector + ?Sized> VenueConnector for Arc<T> {
    async fn connect(&self, venue: &str) -> Result<Arc<dyn VenueConnection>, ConnectionError> {
        (**self).connect(venue).await
    }
}
