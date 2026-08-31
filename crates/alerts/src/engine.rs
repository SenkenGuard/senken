//! [`AlertEngine`] — the running half of, wiring
//! [`AlertStore`] and [`AlertRunner`] to a real [`SubscriptionPool`] per
//! instrument source.
//!
//! Neither [`AlertStore`] nor [`AlertRunner`] know the other exists — this
//! is deliberately the one place that does, so that "an alert never fires
//! during warm-up" and "an alert fires on a closed bar, never a forming
//! one" (both already enforced inside [`crate::AlertEvaluator`]/
//! [`crate::TickBarBuilder`]) are never re-implemented here, only reached
//! through the exact same [`AlertRunner`] a unit test already exercises.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

use senken_subscription::SubscriptionPool;
use tokio::task::AbortHandle;

use crate::id::AlertId;
use crate::runner::AlertRunner;
use crate::store::{AlertRecord, AlertStore};

/// Ties every currently-enabled alert to its own [`AlertRunner`], leasing
/// from whichever [`SubscriptionPool`] serves its instrument's source.
///
/// An alert with no pool for its instrument's source (a venue this build
/// has no live feed for at all) is skipped with a `tracing::warn!` rather
/// than treated as an error — the same "report, do not fail the whole
/// reconciliation" discipline [`AlertStore::all_enabled_for_engine`] already
/// applies to a corrupt row.
///
/// Dropping this engine aborts every alert it is currently running. This
/// matters beyond tidiness: an aborted [`AlertRunner`] drops its own
/// [`senken_subscription::Lease`] as part of being torn down, and — more
/// subtly — it stops a task that would otherwise busy-loop the instant its
/// pool's actor task ends (every future `step()` on a closed pool resolves
/// immediately, with nothing to await), which is exactly what happens when
/// the server that owns both this engine and its pools shuts down and drops
/// them in the same moment.
pub struct AlertEngine {
    store: Arc<AlertStore>,
    pools: HashMap<String, SubscriptionPool>,
    running: Mutex<HashMap<AlertId, AbortHandle>>,
}

impl std::fmt::Debug for AlertEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlertEngine").finish_non_exhaustive()
    }
}

impl AlertEngine {
    /// Loads every currently-enabled alert from `store` and starts running
    /// each one against `pools`, keyed by instrument source id.
    ///
    /// Never fails outright: a `store` read failure is logged and the
    /// engine simply starts with nothing running, the same way a corrupt
    /// individual row is logged and skipped rather than aborting the whole
    /// reconciliation.
    #[must_use]
    pub fn start(store: Arc<AlertStore>, pools: HashMap<String, SubscriptionPool>) -> Self {
        let engine = Self {
            store,
            pools,
            running: Mutex::new(HashMap::new()),
        };
        match engine.store.all_enabled_for_engine() {
            Ok(records) => {
                for record in records {
                    engine.spawn_one(record);
                }
            }
            Err(error) => {
                tracing::error!(
                    %error,
                    "could not load enabled alerts at startup; the engine starts with none running"
                );
            }
        }
        engine
    }

    /// Starts running a freshly created (or re-enabled) alert immediately —
    /// it must not wait for the next server restart to go live.
    pub fn register(&self, record: AlertRecord) {
        self.spawn_one(record);
    }

    /// Stops running an alert (deleted, or disabled) and releases its lease.
    pub fn unregister(&self, id: AlertId) {
        let handle = self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        if let Some(handle) = handle {
            handle.abort();
        }
    }

    fn spawn_one(&self, record: AlertRecord) {
        let AlertRecord {
            id,
            instrument,
            timeframe,
            indicator,
            condition,
            ..
        } = record;

        let Some(pool) = self.pools.get(instrument.source()).cloned() else {
            tracing::warn!(
                alert = %id,
                source = instrument.source(),
                "no live feed for this alert's instrument source in this build; it will not run this session"
            );
            return;
        };
        let concrete = match indicator.build() {
            Ok(concrete) => concrete,
            Err(error) => {
                // `AlertStore::create_alert` already refuses this at the
                // door, so this branch is only reachable for a row this
                // build's indicator catalogue no longer recognises (a
                // downgrade, or a hand-edited database) — reported, not
                // panicked.
                tracing::warn!(alert = %id, %error, "stored indicator spec no longer builds; skipping");
                return;
            }
        };

        let store = Arc::clone(&self.store);
        let task = tokio::spawn(async move {
            let mut runner = match AlertRunner::lease(
                &pool, instrument, timeframe, concrete, condition,
            )
            .await
            {
                Ok(runner) => runner,
                Err(error) => {
                    tracing::warn!(alert = %id, %error, "could not lease this alert's instrument");
                    return;
                }
            };
            loop {
                match runner.step().await {
                    Ok(Some(fired)) => {
                        // A bar's own open time, not a wall-clock read: the
                        // market-data path never reads one (`AGENTS.md`).
                        let fired_at = fired.bar_ts_open.as_millis().div_euclid(1000);
                        if let Err(error) = store.record_fire(id, fired.value, fired_at) {
                            tracing::warn!(alert = %id, %error, "could not record a fire");
                        }
                    }
                    Ok(None) => {}
                    Err(error) => {
                        tracing::warn!(
                            alert = %id,
                            %error,
                            "alert's condition names a field its indicator does not report; stopping"
                        );
                        return;
                    }
                }
            }
        });

        self.running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id, task.abort_handle());
    }
}

impl Drop for AlertEngine {
    fn drop(&mut self) {
        for (_, handle) in self
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .drain()
        {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AlertEngine;
    use crate::condition::{Comparator, Condition, IndicatorField};
    use crate::indicator_spec::IndicatorSpec;
    use crate::store::AlertStore;
    use async_trait::async_trait;
    use senken_identity::{AuthenticatedUser, IdentityStore};
    use senken_marketdata::InstrumentId;
    use senken_series::{BarSpec, BarUnit};
    use senken_subscription::{ConnectionError, SubscriptionPool, VenueConnection, VenueConnector};
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;

    /// Records every subscribe/unsubscribe it receives — the same fake shape
    /// `crate::runner`'s own tests use, reproduced here (private to that
    /// module) rather than shared, so this module never sends a request to
    /// any real venue either.
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

    #[derive(Default)]
    struct FakeConnector {
        opened: Mutex<Vec<Arc<FakeConnection>>>,
        connects: AtomicUsize,
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

    const FAKE_VENUE: &str = "fake-venue";

    fn instrument() -> InstrumentId {
        InstrumentId::new(FAKE_VENUE, "BTCUSDT").unwrap()
    }

    fn above_zero() -> (IndicatorSpec, Condition) {
        (
            IndicatorSpec {
                name: "Sma".to_owned(),
                params: r#"{"period":1}"#.to_owned(),
            },
            Condition {
                field: IndicatorField::Value,
                comparator: Comparator::GreaterThan,
                threshold: 0.0,
            },
        )
    }

    /// A fresh `IdentityStore`/`AlertStore` pair sharing one temp database,
    /// plus the seeded default admin logged in — the exact pattern
    /// `crate::store`'s own tests use for the same setup.
    fn temp_stores() -> (TempDir, IdentityStore, AlertStore, AuthenticatedUser) {
        let dir = TempDir::new().unwrap();
        let identity = IdentityStore::open(dir.path().join("accounts.db")).unwrap();
        let alerts = AlertStore::new(&identity);
        identity
            .set_password(
                senken_identity::DEFAULT_ADMIN_EMAIL,
                "correct horse battery staple",
                None,
            )
            .unwrap();
        let (_uid, token) = identity
            .login(
                senken_identity::DEFAULT_ADMIN_EMAIL,
                "correct horse battery staple",
            )
            .unwrap();
        let admin = identity.resolve_session(token.reveal()).unwrap().unwrap();
        (dir, identity, alerts, admin)
    }

    fn fake_pool() -> (SubscriptionPool, Arc<FakeConnector>) {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new(FAKE_VENUE, Arc::clone(&connector));
        (pool, connector)
    }

    /// the reason to exist: an alert already `enabled` in the
    /// store when the *server restarts* — not only one created through
    /// [`AlertEngine::register`] during the engine's own lifetime — must
    /// still be running, because [`AlertEngine::start`] is what a fresh
    /// server process calls at boot.
    #[tokio::test]
    async fn start_leases_an_alert_that_was_already_enabled_in_the_store() {
        let (_dir, _identity, alerts, admin) = temp_stores();
        let (indicator, condition) = above_zero();
        alerts
            .create_alert(
                &admin,
                &instrument(),
                BarSpec::new(1, BarUnit::Minute),
                &indicator,
                condition,
            )
            .unwrap();

        let (pool, connector) = fake_pool();
        let engine = AlertEngine::start(
            Arc::new(alerts),
            HashMap::from([(FAKE_VENUE.to_owned(), pool.clone())]),
        );
        pool.flush().await;

        let opened = connector
            .opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            opened.len(),
            1,
            "start() must dial the fake venue exactly once"
        );
        assert_eq!(
            opened[0]
                .subscribed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[instrument()],
            "an alert already enabled in the store before start() must still be leased"
        );

        drop(engine);
    }

    /// R6/`AlertStore::all_enabled_for_engine`'s own "report, do not fail the
    /// whole reconciliation" discipline, exercised at the engine level: one
    /// alert names a source this build has no pool for at all, and must be
    /// skipped — logged, not panicked — while every other alert still runs.
    #[tokio::test]
    async fn an_alert_with_no_pool_for_its_source_is_skipped_not_fatal() {
        let (_dir, _identity, alerts, admin) = temp_stores();
        let (indicator, condition) = above_zero();
        // A source no pool below will ever serve.
        let orphan = InstrumentId::new("no-live-feed-venue", "ETHUSDT").unwrap();
        alerts
            .create_alert(
                &admin,
                &orphan,
                BarSpec::new(1, BarUnit::Minute),
                &indicator.clone(),
                condition,
            )
            .unwrap();
        alerts
            .create_alert(
                &admin,
                &instrument(),
                BarSpec::new(1, BarUnit::Minute),
                &indicator,
                condition,
            )
            .unwrap();

        let (pool, connector) = fake_pool();
        let engine = AlertEngine::start(
            Arc::new(alerts),
            HashMap::from([(FAKE_VENUE.to_owned(), pool.clone())]),
        );
        pool.flush().await;

        let opened = connector
            .opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            opened.len(),
            1,
            "the alert with a real pool must still run despite the orphaned one existing"
        );
        assert_eq!(
            opened[0]
                .subscribed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[instrument()],
            "only the alert whose source has a pool is ever subscribed"
        );

        drop(engine);
    }

    /// `register`/`unregister` (the create/delete wiring,
    /// `crates/api/src/alert_handlers.rs`): registering starts a lease
    /// immediately, and unregistering releases it — proven here at the
    /// engine level, independent of the HTTP layer that calls them.
    #[tokio::test]
    async fn unregister_releases_the_lease_register_took() {
        let (_dir, _identity, alerts, admin) = temp_stores();
        let alerts = Arc::new(alerts);

        // `start()` runs first, against a store that as yet holds no alerts
        // at all — the same "created after the engine is already running"
        // shape `alert_handlers::create_alert` produces over HTTP. Starting
        // first (rather than after `create_alert`, which would leave the
        // alert already `enabled` for `start()` to also pick up) is what
        // keeps the lease below `register`'s doing alone, not a second,
        // duplicate one from `start`'s own reconciliation.
        let (pool, connector) = fake_pool();
        let engine = AlertEngine::start(
            Arc::clone(&alerts),
            HashMap::from([(FAKE_VENUE.to_owned(), pool.clone())]),
        );

        let (indicator, condition) = above_zero();
        let id = alerts
            .create_alert(
                &admin,
                &instrument(),
                BarSpec::new(1, BarUnit::Minute),
                &indicator,
                condition,
            )
            .unwrap();
        let record = alerts.get_alert(&admin, id).unwrap();
        engine.register(record);
        pool.flush().await;
        {
            let opened = connector
                .opened
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            assert_eq!(opened.len(), 1);
            assert_eq!(
                opened[0]
                    .subscribed
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .as_slice(),
                &[instrument()],
                "register() must lease immediately, not wait for a restart"
            );
        }

        engine.unregister(id);
        pool.flush().await;
        let opened = connector
            .opened
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(
            opened[0]
                .unsubscribed
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_slice(),
            &[instrument()],
            "unregister() must release the exact lease register() took"
        );

        drop(engine);
    }
}
