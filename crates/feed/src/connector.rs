//! [`WsVenueConnector`] — opens a new [`WsVenueConnection`] on demand, the
//! only entry point [`senken_subscription::SubscriptionPool`] ever calls
//! itself.

use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use senken_subscription::{ConnectionError, SubscriptionPool, VenueConnection, VenueConnector};
use senken_venue::LimitGroup;

use crate::connection::WsVenueConnection;
use senken_subscription::VenueProtocol;

/// Opens [`WsVenueConnection`]s for one venue's protocol `P`, sharing one
/// [`LimitGroup`] budget and publishing every decoded price into one
/// [`SubscriptionPool`].
///
/// # Construction is two steps, not one
///
/// A [`WsVenueConnection`] needs a [`SubscriptionPool`] handle to call
/// [`SubscriptionPool::publish`]/[`SubscriptionPool::reconnected`] on — but
/// [`SubscriptionPool::new`]/[`with_cap`](senken_subscription::SubscriptionPool::with_cap)
/// need a [`VenueConnector`] *before* they can return a pool at all. Neither
/// side of that cycle can be built first in the usual way, so this type
/// holds its pool behind an [`OnceLock`] instead of owning one outright:
///
/// ```ignore
/// let connector = WsVenueConnector::new(protocol, group);
/// let pool = SubscriptionPool::new("okx", connector.clone());
/// connector.bind_pool(pool.clone()); // before any `pool.lease(...)` call
/// ```
///
/// This is safe because [`SubscriptionPool::new`] never calls
/// [`VenueConnector::connect`] itself — only a later
/// [`SubscriptionPool::lease`] does (see that crate's own actor) — so as
/// long as [`bind_pool`](Self::bind_pool) runs before the first `lease`
/// call, no connection is ever opened with an unbound pool.
///
/// Cheap to clone, like every other handle in this project's venue plumbing
/// (`LimitGroup`, `SubscriptionPool`): every clone opens connections under
/// the same budget, publishes into the same pool once bound, and shares the
/// same not-yet-bound state until then.
pub struct WsVenueConnector<P: VenueProtocol + ?Sized> {
    protocol: Arc<P>,
    group: LimitGroup,
    pool: Arc<OnceLock<SubscriptionPool>>,
}

// Written by hand rather than `#[derive(Clone)]`: the derive macro adds a
// `P: Clone` bound even though only `Arc<P>` is ever stored, which would
// force every `VenueProtocol` implementation to also implement `Clone` for
// no reason this type actually needs.
impl<P: VenueProtocol + ?Sized> Clone for WsVenueConnector<P> {
    fn clone(&self) -> Self {
        Self {
            protocol: Arc::clone(&self.protocol),
            group: self.group.clone(),
            pool: Arc::clone(&self.pool),
        }
    }
}

impl<P: VenueProtocol + ?Sized> WsVenueConnector<P> {
    /// A connector for `protocol`, dialling through `group`'s shared
    /// budget. Not yet usable to open a connection — call
    /// [`bind_pool`](Self::bind_pool) with the pool this connector is being
    /// built for before that pool's first [`SubscriptionPool::lease`] call.
    #[must_use]
    pub fn new(protocol: P, group: LimitGroup) -> Self
    where
        P: Sized,
    {
        Self::from_arc(Arc::new(protocol), group)
    }

    /// As [`new`](Self::new), but over a protocol the caller already holds
    /// behind an [`Arc`] — including an `Arc<dyn VenueProtocol>`, which is
    /// what a plugin's `FeedSource` hands the runtime. Without this,
    /// `WsVenueConnector` could only ever be built over a statically known
    /// protocol type, which is exactly what a registry of plugins cannot
    /// provide.
    #[must_use]
    pub fn from_arc(protocol: Arc<P>, group: LimitGroup) -> Self {
        Self {
            protocol,
            group,
            pool: Arc::new(OnceLock::new()),
        }
    }

    /// Wires this connector to the pool it opens connections for.
    ///
    /// Every clone of this connector shares the same not-yet-bound cell, so
    /// calling this once — right after the matching
    /// [`SubscriptionPool::new`]/`with_cap` call returns — is enough
    /// regardless of how many clones of this connector end up inside that
    /// pool's actor or elsewhere.
    ///
    /// # Panics
    /// If called more than once. A [`WsVenueConnector`] serves exactly one
    /// pool for its entire life — this is a construction-order programmer
    /// error, not a runtime condition any caller should need to recover
    /// from.
    pub fn bind_pool(&self, pool: SubscriptionPool) {
        self.pool
            .set(pool)
            .unwrap_or_else(|_| panic!("WsVenueConnector::bind_pool called more than once"));
    }

    /// The pool this connector was bound to.
    ///
    /// # Panics
    /// If [`bind_pool`](Self::bind_pool) has not been called yet — see this
    /// type's own docs for the required construction order.
    pub(crate) fn pool(&self) -> &SubscriptionPool {
        self.pool
            .get()
            .expect("WsVenueConnector::bind_pool must be called before connecting")
    }
}

#[async_trait]
impl<P: VenueProtocol + ?Sized> VenueConnector for WsVenueConnector<P> {
    async fn connect(&self, venue: &str) -> Result<Arc<dyn VenueConnection>, ConnectionError> {
        if venue != self.protocol.venue() {
            // A caller bug, not a venue failure: this connector was built
            // for one venue's protocol and the pool it serves must be the
            // one constructed with the matching name (`SubscriptionPool::venue`
            // is set once, at construction, to the same string).
            return Err(ConnectionError::new(format!(
                "this connector serves \"{}\", not \"{venue}\"",
                self.protocol.venue()
            )));
        }
        let connection = WsVenueConnection::connect(
            Arc::clone(&self.protocol),
            self.group.clone(),
            self.pool().clone(),
        )
        .await?;
        Ok(connection as Arc<dyn VenueConnection>)
    }
}
