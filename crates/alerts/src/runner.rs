//! [`AlertRunner`] — the live half of an alert.
//!
//! This is where its central architectural claim actually lives in code:
//! an [`AlertRunner`] holds its **own** [`Lease`] on the series it watches,
//! obtained the same way a chart pane, a watchlist row or a position would
//! (`SubscriptionPool::lease`). It shares nothing with whatever chart, if
//! any, happened to be open when the alert was created — there is no
//! back-reference to a chart or a pane anywhere in this type. Dropping a
//! chart's own lease has no effect whatsoever on an `AlertRunner`'s: the
//! pool only unsubscribes from the venue once *every* lease on an
//! instrument — the chart's and the alert's alike — has been dropped. See
//! `runner_tests` for the constructed proof.

use senken_marketdata::InstrumentId;
use senken_series::BarSpec;
use senken_subscription::{Lease, PoolError, PriceUpdate, SubscriptionPool};
use tokio::sync::watch;

use crate::bar_builder::TickBarBuilder;
use crate::condition::Condition;
use crate::error::IndicatorSpecError;
use crate::evaluator::{AlertEvaluator, Fired};
use crate::indicator_spec::ConcreteIndicator;

/// The live evaluation half of one alert: a series lease, a live bar
/// builder folding that lease's ticks, and the evaluator deciding whether
/// each closed bar fires.
///
/// Constructing one is the whole of the "leases the series
/// itself" — [`AlertRunner::lease`] calls
/// [`SubscriptionPool::lease`] exactly the way a chart pane would, and
/// holds the returned guard for as long as the runner itself lives, which
/// in real use is the alert's own lifetime, not any chart's.
#[derive(Debug)]
pub struct AlertRunner {
    // Never read after construction except via `instrument()` — its entire
    // job is to exist for as long as this runner does, keeping the venue
    // subscription alive independent of whatever chart pane leases (if any)
    // happen to come and go.
    lease: Lease,
    updates: watch::Receiver<Option<PriceUpdate>>,
    builder: TickBarBuilder,
    evaluator: AlertEvaluator,
}

impl AlertRunner {
    /// Leases `instrument` from `pool` and builds a runner around it.
    ///
    /// # Errors
    /// Whatever [`SubscriptionPool::lease`] itself returns.
    pub async fn lease(
        pool: &SubscriptionPool,
        instrument: InstrumentId,
        spec: BarSpec,
        indicator: ConcreteIndicator,
        condition: Condition,
    ) -> Result<Self, PoolError> {
        let lease = pool.lease(instrument).await?;
        Ok(Self::from_lease(lease, spec, indicator, condition))
    }

    /// As [`lease`](Self::lease), but from an already-obtained [`Lease`] —
    /// the seam tests use to hold their own separate "chart" lease on the
    /// same instrument alongside the one this runner takes for itself.
    #[must_use]
    pub fn from_lease(
        lease: Lease,
        spec: BarSpec,
        indicator: ConcreteIndicator,
        condition: Condition,
    ) -> Self {
        let updates = lease.updates();
        Self {
            lease,
            updates,
            builder: TickBarBuilder::new(spec),
            evaluator: AlertEvaluator::new(indicator, condition),
        }
    }

    /// The instrument this runner leases.
    #[must_use]
    pub fn instrument(&self) -> &InstrumentId {
        self.lease.instrument()
    }

    /// Waits for the next price update on this runner's lease, folds it
    /// into whatever bar is currently forming, and evaluates the alert's
    /// condition if that tick closed a bar.
    ///
    /// Returns `Ok(None)` for every tick that does not close a bar (the
    /// ordinary case) or once the pool itself is gone (its actor task ended
    ///   — nothing left to watch); `Ok(Some(_))` exactly on the closed bar
    /// where the condition newly became true.
    ///
    /// # Errors
    /// [`IndicatorSpecError::FieldNotReported`] if this alert's condition
    /// names a field its indicator does not report — a configuration
    /// mistake, not a runtime one, but only detectable once a bar actually
    /// closes.
    pub async fn step(&mut self) -> Result<Option<Fired>, IndicatorSpecError> {
        loop {
            if self.updates.changed().await.is_err() {
                // The pool's actor task is gone — nothing left to watch, and
                // nothing more this runner can usefully do.
                return Ok(None);
            }
            let Some(tick) = *self.updates.borrow() else {
                // A watch channel's initial value before any tick has ever
                // arrived for this instrument.
                continue;
            };
            if let Some(bar) = self.builder.push(&tick) {
                return self.evaluator.on_closed_bar(&bar);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AlertRunner;
    use crate::condition::{Comparator, Condition, IndicatorField};
    use crate::indicator_spec::ConcreteIndicator;
    use async_trait::async_trait;
    use senken_core::UnixNanos;
    use senken_marketdata::InstrumentId;
    use senken_series::{BarSpec, BarUnit};
    use senken_subscription::{
        ConnectionError, PriceUpdate, SubscriptionPool, VenueConnection, VenueConnector,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    /// Records every subscribe/unsubscribe it receives — the same fake
    /// shape `senken-subscription`'s own tests use, reproduced here rather
    /// than shared (it is private to that crate's test module) so this
    /// crate never sends a request to any real venue either.
    #[derive(Default)]
    struct FakeConnection {
        unsubscribed: Mutex<Vec<InstrumentId>>,
    }

    #[async_trait]
    impl VenueConnection for FakeConnection {
        async fn subscribe(&self, _instrument: &InstrumentId) -> Result<(), ConnectionError> {
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

    fn instrument() -> InstrumentId {
        InstrumentId::new("fake-venue", "BTCUSDT").unwrap()
    }

    fn tick(secs: i64, price: i64) -> PriceUpdate {
        PriceUpdate {
            ts: UnixNanos::from_secs(secs).unwrap(),
            price,
            price_scale: 2,
            qty: senken_series::Volume::Real(0),
            qty_scale: 0,
        }
    }

    /// `GreaterThan`, not `CrossesAbove`: a crossing condition needs two
    /// *initialized* readings before it can ever fire (there is nothing to
    /// have crossed from on the very first one — see `evaluator.rs`'s own
    /// tests, which cover that distinction directly). These integration
    /// tests are about the lease/lifetime and forming-vs-closed properties,
    /// not about crossing detection, so the simplest condition that can
    /// fire on a single closed bar is the right one to use here.
    fn above_100() -> (ConcreteIndicator, Condition) {
        (
            ConcreteIndicator::build("Sma", r#"{"period":1}"#).unwrap(),
            Condition {
                field: IndicatorField::Value,
                comparator: Comparator::GreaterThan,
                threshold: 100.0,
            },
        )
    }

    /// The property this exists for: an alert outlives the chart that
    /// created it.
    ///
    /// A "chart" here is nothing more than another lease on the same
    /// instrument — precisely because an alert must not
    /// be built as a hidden chart session, there is no `Chart` type to
    /// construct; a plain lease *is* everything a chart pane contributes to
    /// this scenario ("chart panes ... are all just leaseholders"). Dropping it and continuing to observe the alert
    /// still working is the whole proof.
    #[tokio::test]
    async fn an_alert_outlives_the_chart_that_created_it() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));

        // The chart opens a pane on BTCUSDT — just a lease.
        let chart_pane_lease = pool.lease(instrument()).await.unwrap();

        // The alert is created independently and leases the *same*
        // instrument for itself — never through the chart, never sharing
        // its lease.
        let (indicator, condition) = above_100();
        let alert_lease = pool.lease(instrument()).await.unwrap();
        let mut runner = AlertRunner::from_lease(
            alert_lease,
            BarSpec::new(1, BarUnit::Minute),
            indicator,
            condition,
        );

        // The chart is closed.
        drop(chart_pane_lease);
        pool.flush().await;
        // An owned clone, not a held `MutexGuard` — this is read again after
        // several `.await` points below, which a lock guard cannot survive.
        let shard = connector.opened.lock().unwrap()[0].clone();
        assert!(
            shard.unsubscribed.lock().unwrap().is_empty(),
            "the alert's own lease must keep the venue subscription alive \
             after the chart's lease is dropped"
        );

        // Prices keep arriving after the chart is gone, and the alert keeps
        // reacting to them exactly as it would have before. `step()` is
        // spawned so it can genuinely await each tick in turn — the watch
        // channel underneath only ever holds the *latest* value, so two
        // `publish` calls made back-to-back before anything ever reads the
        // channel would coalesce into one and the first (bucket-opening)
        // tick would be lost. Interleaving with a publishing tick in
        // between each `sleep` gives the spawned task room to actually
        // observe both.
        let step_task = tokio::spawn(async move { (runner.step().await, runner) });
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        pool.publish(instrument(), tick(0, 150)); // opens minute 0 at 150 (already above 100)
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        pool.publish(instrument(), tick(65, 5)); // opens minute 1 — closes minute 0 with close = 150
        let (result, runner) = tokio::time::timeout(std::time::Duration::from_secs(5), step_task)
            .await
            .expect("the runner must observe both ticks well within this generous timeout")
            .unwrap();
        let fired = result
            .unwrap()
            .expect("the alert must still fire after the chart that created it has closed");
        assert_eq!(
            fired.value.to_bits(),
            150.0_f64.to_bits(),
            "the closed bar's value is minute 0's own close (150), not minute 1's opening tick"
        );

        drop(runner);
        pool.flush().await;
        assert_eq!(
            shard.unsubscribed.lock().unwrap().as_slice(),
            &[instrument()],
            "only now, with the alert's own lease also dropped, does the venue see an unsubscribe"
        );
    }

    #[tokio::test]
    async fn a_runner_does_not_fire_on_a_still_forming_bucket() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));
        let (indicator, condition) = above_100();
        let lease = pool.lease(instrument()).await.unwrap();
        let mut runner = AlertRunner::from_lease(
            lease,
            BarSpec::new(1, BarUnit::Minute),
            indicator,
            condition,
        );

        // Two ticks in the same forming minute-0 bucket, the second one
        // already satisfying the condition on its raw price alone.
        pool.publish(instrument(), tick(0, 90));
        pool.flush().await;
        pool.publish(instrument(), tick(30, 150));
        pool.flush().await;

        // Neither tick closed a bucket, so `step` must still be waiting —
        // proven by racing it against a short timeout rather than blocking
        // the test forever if this regresses.
        let outcome =
            tokio::time::timeout(std::time::Duration::from_millis(50), runner.step()).await;
        assert!(
            outcome.is_err(),
            "no bucket has closed yet — the runner must not have produced an outcome"
        );
    }
}
