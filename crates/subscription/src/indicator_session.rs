//! Live indicator sessions: an incremental indicator kept warm over a
//! [`Lease`], advanced once per closed bar and re-read cheaply on every
//! tick in between.
//!
//! # The problem this replaces
//!
//! `senken-indicators` is built so a new bar updates an indicator's
//! existing state rather than triggering a recompute over the bars behind
//! it — but nothing before this module actually held that state open
//! across ticks. A stateless HTTP endpoint has to replay a whole range from
//! scratch on every call, which is affordable once and unaffordable once a
//! second, which is exactly how a chart drawing a live indicator used it.
//!
//! [`IndicatorEngine`] is the fix: it holds one [`ConcreteIndicator`]'s
//! confirmed state plus a [`TickBarBuilder`] folding raw ticks into bars,
//! and separates two operations that cost very differently —
//! [`advance`](IndicatorEngine::advance) is O(1) and only ever moves the
//! confirmed state forward one closed bar at a time;
//! [`rebase`](IndicatorEngine::rebase) is O(n) and is the only way the
//! confirmed state's coverage can move backward (more history added to the
//! left, or a parameter change). A tick that has not yet closed a bar is
//! never fed to the confirmed state at all — [`IndicatorEngine::on_tick`]
//! answers it from [`ConcreteIndicator::snapshot`] instead, a cheap clone
//! that can be advanced and read without disturbing the confirmed
//! indicator underneath it.
//!
//! [`IndicatorSession`] is the live half: it holds its own [`Lease`], the
//! same way [`crate::TickBarBuilder`]'s other consumers do, and republishes
//! every [`IndicatorReading`] the engine produces onto a [`watch`] channel
//! so any number of consumers can subscribe without re-driving the
//! computation themselves. [`IndicatorSessionRegistry`] deduplicates
//! sessions that share the same `(instrument, spec, indicator, params)` —
//! deliberately **not** who is asking, see that type's own docs — the same
//! reference-counted, `Drop`-released shape [`SubscriptionPool`] and
//! [`Lease`] already use.

use std::collections::HashMap;
use std::sync::{Arc, Weak};

use senken_core::UnixNanos;
use senken_indicators::{ConcreteIndicator, IndicatorField};
use senken_marketdata::InstrumentId;
use senken_series::{Bar, BarSpec};
use tokio::sync::{Mutex, watch};
use tokio::task::AbortHandle;

use crate::pool::{Lease, PoolError, SubscriptionPool};
use crate::price::PriceUpdate;
use crate::session::TickBarBuilder;

/// One indicator update ready to publish to a live session's consumers.
///
/// Carries every field the wrapped indicator reports (an `Sma` reports one,
/// a `Macd` three) rather than one reading per field, so a single closed
/// bar or a single tick produces exactly one [`IndicatorReading`] — the
/// wire layer decides whether to flatten that into one frame per field.
#[derive(Debug, Clone, PartialEq)]
pub struct IndicatorReading {
    /// The bar this reading was computed from — the bar that just closed
    /// for a confirmed reading, the bucket still forming for a provisional
    /// one.
    pub ts_open: UnixNanos,
    /// `false` once the bar this reading covers has actually closed;
    /// `true` while it is still forming. A chart draws both; nothing else
    /// in this codebase may ever act on `true` — an alert firing on a
    /// provisional reading is exactly the false positive
    /// `senken-alerts`' own forming-bucket discipline exists to prevent
    /// (see `senken_alerts::AlertRunner::step`, which never even
    /// constructs a reading for a still-forming bucket in the first
    /// place).
    pub provisional: bool,
    /// One `(field, value)` pair per field this indicator reports, in
    /// [`ConcreteIndicator::reported_fields`] order.
    pub values: Vec<(IndicatorField, f64)>,
}

/// Reads every field `indicator` reports into an [`IndicatorReading`], or
/// `None` if `indicator` is not yet [`initialized`](ConcreteIndicator::initialized)
/// — the same warm-up gate `senken_indicators::Indicator::initialized`
/// documents: an indicator's first few values are a warm-up artefact, not
/// one a consumer should ever see, provisional or not.
fn reading_from(
    indicator: &ConcreteIndicator,
    ts_open: UnixNanos,
    provisional: bool,
) -> Option<IndicatorReading> {
    if !indicator.initialized() {
        return None;
    }
    let values = indicator
        .reported_fields()
        .iter()
        .filter_map(|&field| indicator.read(field).ok().map(|value| (field, value)))
        .collect();
    Some(IndicatorReading {
        ts_open,
        provisional,
        values,
    })
}

/// The pure computation a live indicator session drives: one
/// [`ConcreteIndicator`]'s confirmed state, plus the [`TickBarBuilder`]
/// folding raw ticks into the bars that state advances on.
///
/// Nothing here touches a [`Lease`] or a channel — every method is a plain
/// function of its inputs, which is what makes `advance`/`rebase`/`on_tick`
/// independently unit-testable without a running Tokio runtime or a fake
/// venue.
#[derive(Debug)]
pub struct IndicatorEngine {
    builder: TickBarBuilder,
    canonical: ConcreteIndicator,
}

impl IndicatorEngine {
    /// Builds an engine over `indicator`, with no bars handled yet — call
    /// [`rebase`](Self::rebase) with whatever history is already available
    /// before treating this engine's readings as meaningful.
    #[must_use]
    pub fn new(spec: BarSpec, indicator: ConcreteIndicator) -> Self {
        Self {
            builder: TickBarBuilder::new(spec),
            canonical: indicator,
        }
    }

    /// Feeds one already-closed bar into the confirmed indicator state —
    /// O(1), the only way the confirmed state is ever supposed to move
    /// forward. Returns the resulting confirmed reading, or `None` while
    /// still warming up.
    pub fn advance(&mut self, bar: &Bar) -> Option<IndicatorReading> {
        self.canonical.handle_bar(bar);
        reading_from(&self.canonical, bar.ts_open, false)
    }

    /// Replays `history` (oldest first) into a freshly reset indicator —
    /// O(n), and the only way this engine's coverage can move backward:
    /// more history added to the left, or the indicator's own parameters
    /// changing. `history` must be exactly what a fresh load over the same
    /// range would see, or the two diverge.
    pub fn rebase(&mut self, history: &[Bar]) {
        self.canonical.reset();
        for bar in history {
            self.canonical.handle_bar(bar);
        }
    }

    /// A read-only look at what the confirmed indicator would report if
    /// `forming` were its next bar. Never mutates `self` —
    /// [`ConcreteIndicator::snapshot`] is cloned, advanced, and read, and
    /// the clone is then dropped; the confirmed state underneath is
    /// exactly as it was before this call.
    #[must_use]
    pub fn provisional(&self, forming: &Bar) -> Option<IndicatorReading> {
        let mut probe = self.canonical.snapshot();
        probe.handle_bar(forming);
        reading_from(&probe, forming.ts_open, true)
    }

    /// Folds one tick through this engine's [`TickBarBuilder`] and returns
    /// whatever reading it produces: a confirmed one if the tick closed a
    /// bucket ([`advance`](Self::advance), O(1)), a provisional one from
    /// the bucket still forming afterward ([`provisional`](Self::provisional),
    /// also O(1)), or `None` if neither has enough bars yet to be
    /// initialized.
    pub fn on_tick(&mut self, tick: &PriceUpdate) -> Option<IndicatorReading> {
        if let Some(closed) = self.builder.push(tick) {
            return self.advance(&closed);
        }
        let forming = self.builder.forming()?;
        self.provisional(&forming)
    }
}

/// `(instrument, spec, indicator, params)` — the dedup key two chart panes
/// with identical live indicators share one session under.
///
/// Deliberately **does not** name who is asking. That is correct today
/// because every field an indicator in this crate ever reads comes from
/// [`senken_series::Bar`] — global market data, never tenanted per user
/// (`AGENTS.md`: "Market data is global"). [`ConcreteIndicator`] is a
/// closed enum over exactly the ten built-ins, and not one of their
/// [`senken_indicators::Indicator::handle_bar`] signatures accepts
/// anything but a `Bar` — there is no field on this key, and no way to
/// plumb one through [`IndicatorSessionRegistry::get_or_create`]'s
/// signature, that could carry a per-user value today. That is an argument
/// from what this key's fields are, not a compiler-enforced seal: the day
/// an indicator needs a per-user input, this struct must grow a field for
/// it (or stop being shared at all), and that is a visible, reviewable
/// change to this exact type — not a silent cross-user leak through a key
/// that already happened to have room for one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct IndicatorSessionKey {
    instrument: InstrumentId,
    spec: BarSpec,
    indicator: String,
    params: String,
}

impl IndicatorSessionKey {
    /// Builds a dedup key from a live indicator's identifying inputs.
    #[must_use]
    pub fn new(
        instrument: InstrumentId,
        spec: BarSpec,
        indicator: impl Into<String>,
        params: impl Into<String>,
    ) -> Self {
        Self {
            instrument,
            spec,
            indicator: indicator.into(),
            params: params.into(),
        }
    }
}

/// Drives one [`IndicatorEngine`] off a [`Lease`]'s tick stream for as long
/// as anything holds an [`IndicatorSessionHandle`] to it.
///
/// Reads whatever tick is already on the channel before waiting for the
/// next one: a tick published between [`Lease::updates`] returning and this
/// task's first read would otherwise never be seen, since a fresh
/// [`watch::Receiver`] marks the value present at its creation as already
/// seen.
async fn drive(
    mut updates: watch::Receiver<Option<PriceUpdate>>,
    mut engine: IndicatorEngine,
    output: watch::Sender<Option<IndicatorReading>>,
) {
    loop {
        let current = *updates.borrow_and_update();
        if let Some(tick) = current
            && let Some(reading) = engine.on_tick(&tick)
            && output.send(Some(reading)).is_err()
        {
            return; // no handle is listening any more
        }
        if updates.changed().await.is_err() {
            return; // the pool's actor task is gone; nothing left to drive
        }
    }
}

/// The live half of one shared indicator session: an [`IndicatorEngine`]
/// driven off its own [`Lease`], for as long as any
/// [`IndicatorSessionHandle`] built from the same
/// [`IndicatorSessionRegistry::get_or_create`] call is still alive.
struct IndicatorSessionInner {
    // Held only to keep the venue subscription alive for as long as this
    // session exists, and to answer `IndicatorSessionHandle::instrument` —
    // exactly the pattern `senken_alerts::AlertRunner` already documents
    // for its own lease.
    lease: Lease,
    updates: watch::Sender<Option<IndicatorReading>>,
    task: AbortHandle,
}

impl Drop for IndicatorSessionInner {
    fn drop(&mut self) {
        tracing::debug!(
            instrument = %self.lease.instrument(),
            "retiring a live indicator session — its last holder just dropped"
        );
        // Stops the background task driving this session; `lease` is then
        // dropped as an ordinary field immediately after this method
        // returns, releasing the venue subscription the same way any other
        // dropped `Lease` does.
        self.task.abort();
    }
}

/// A reference-counted handle onto one shared, internal session record.
///
/// Two chart panes leasing the same `(instrument, spec, indicator,
/// params)` hold two [`IndicatorSessionHandle`]s onto the *same* `Arc`, the
/// same way two leaseholders on one instrument share one
/// [`crate::pool::SubscriptionPool`] actor's watch channel for it. The
/// underlying session — its task and its [`Lease`] — is released only once
/// every handle onto it has been dropped; there is no explicit "close"
/// method, matching [`Lease`]'s own `Drop`-only shape.
#[must_use = "dropping this immediately releases this handle's share of the session"]
pub struct IndicatorSessionHandle {
    inner: Arc<IndicatorSessionInner>,
}

impl IndicatorSessionHandle {
    /// The instrument this session's own [`Lease`] claims — read straight
    /// off that lease so this handle never needs a second copy of it.
    #[must_use]
    pub fn instrument(&self) -> &InstrumentId {
        self.inner.lease.instrument()
    }

    /// A receiver for this session's live readings — `None` until the
    /// first one is produced, `Some` after. Mints an independent receiver
    /// starting from the channel's current value, exactly like
    /// [`Lease::updates`].
    #[must_use]
    pub fn updates(&self) -> watch::Receiver<Option<IndicatorReading>> {
        self.inner.updates.subscribe()
    }

    /// How many [`IndicatorSessionHandle`]s (including this one) currently
    /// share the underlying session — the "holders per session" metric
    /// this jalur's plan calls for, read directly off the `Arc` doing the
    /// reference counting rather than a second counter that could drift
    /// from it.
    #[must_use]
    pub fn holder_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }
}

impl std::fmt::Debug for IndicatorSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndicatorSessionHandle")
            .field("holders", &self.holder_count())
            .finish_non_exhaustive()
    }
}

/// Deduplicates live indicator sessions by [`IndicatorSessionKey`],
/// reference-counted and released on `Drop` — the session-level analogue
/// of [`SubscriptionPool`] deduplicating leases by instrument.
///
/// One process-wide registry is expected per server, the same way one
/// [`SubscriptionPool`] is built per venue: callers that want two chart
/// panes with identical live indicators to share one session must go
/// through the same [`IndicatorSessionRegistry`] instance.
#[derive(Default)]
pub struct IndicatorSessionRegistry {
    sessions: Mutex<HashMap<IndicatorSessionKey, Weak<IndicatorSessionInner>>>,
}

impl IndicatorSessionRegistry {
    /// Returns a handle onto the live session for `key`, creating one (and
    /// leasing `instrument` from `pool`) if none is currently live.
    ///
    /// `indicator` and `warm_up` are only used the first time a session is
    /// created for `key`: the second caller within a session's lifetime
    /// gets back a handle onto the *existing* engine's state, already
    /// warmed up, without re-running its own warm-up. The whole lock is
    /// held across leasing the instrument and warming up the indicator, so
    /// two concurrent calls for the same brand-new key cannot both create a
    /// session — the same "one at a time" discipline
    /// [`SubscriptionPool`]'s own actor gives lease creation.
    ///
    /// # Errors
    /// Whatever [`SubscriptionPool::lease`] itself returns.
    pub async fn get_or_create(
        &self,
        pool: &SubscriptionPool,
        key: IndicatorSessionKey,
        indicator: ConcreteIndicator,
        warm_up: &[Bar],
    ) -> Result<IndicatorSessionHandle, PoolError> {
        let mut sessions = self.sessions.lock().await;
        if let Some(inner) = sessions.get(&key).and_then(Weak::upgrade) {
            tracing::debug!(
                instrument = %key.instrument,
                indicator = %key.indicator,
                holders = Arc::strong_count(&inner) + 1,
                "joining an existing live indicator session"
            );
            return Ok(IndicatorSessionHandle { inner });
        }

        let lease = pool.lease(key.instrument.clone()).await?;
        let price_updates = lease.updates();

        let mut engine = IndicatorEngine::new(key.spec, indicator);
        engine.rebase(warm_up);

        let (tx, _initial_rx) = watch::channel(None);
        let task = tokio::spawn(drive(price_updates, engine, tx.clone()));

        let inner = Arc::new(IndicatorSessionInner {
            lease,
            updates: tx,
            task: task.abort_handle(),
        });
        sessions.insert(key.clone(), Arc::downgrade(&inner));
        sessions.retain(|_, weak| weak.strong_count() > 0);
        tracing::debug!(
            instrument = %key.instrument,
            indicator = %key.indicator,
            live_sessions = sessions.len(),
            "opened a new live indicator session"
        );
        Ok(IndicatorSessionHandle { inner })
    }

    /// How many sessions are currently live — the "sessions hidup" metric
    /// this jalur's plan calls for. Also prunes dead entries whose last
    /// handle has already been dropped, so this never overcounts a session
    /// nothing holds any more.
    pub async fn live_sessions(&self) -> usize {
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, weak| weak.strong_count() > 0);
        sessions.len()
    }
}

#[cfg(test)]
mod engine_tests {
    use super::{IndicatorEngine, IndicatorField};
    use crate::price::PriceUpdate;
    use senken_core::UnixNanos;
    use senken_indicators::ConcreteIndicator;
    use senken_series::{BarSpec, BarUnit, Volume};

    fn spec() -> BarSpec {
        BarSpec::new(1, BarUnit::Minute)
    }

    fn sma1() -> ConcreteIndicator {
        ConcreteIndicator::build("Sma", r#"{"period":1}"#).unwrap()
    }

    fn tick(secs: i64, price: i64) -> PriceUpdate {
        PriceUpdate {
            ts: UnixNanos::from_secs(secs).unwrap(),
            price,
            price_scale: 2,
            qty: Volume::Real(1),
            qty_scale: 0,
        }
    }

    /// The bukti this whole module exists to satisfy: taking a snapshot and
    /// advancing it must never move the confirmed engine underneath it.
    #[test]
    fn provisional_reads_never_mutate_the_confirmed_engine() {
        // `Sma(2)` is a sliding window over the *last two* bars it was fed
        // — so if `provisional`'s snapshot leaked into the confirmed
        // state, the confirmed value after the real second bar closes
        // would be the average of the provisional probe's price and that
        // second bar, rather than the two genuinely closed bars.
        let sma2 = senken_indicators::ConcreteIndicator::build("Sma", r#"{"period":2}"#).unwrap();
        let mut engine = IndicatorEngine::new(spec(), sma2);

        engine.advance(&super::tests_support::bar(0, 10));
        // A provisional probe with a wildly different price than anything
        // that ever actually closes — if this leaked into the confirmed
        // state, it would be impossible to miss in the assertion below.
        let probe = engine.provisional(&super::tests_support::bar(60, 990));
        assert!(
            probe.unwrap().provisional,
            "the probe itself must still initialize (two bars deep) and be marked provisional"
        );

        let confirmed = engine
            .advance(&super::tests_support::bar(60, 20))
            .expect("two genuinely closed bars must initialize a period-2 SMA");
        assert_eq!(
            confirmed.values,
            vec![(IndicatorField::Value, 15.0)],
            "the confirmed average of the two real closes (10, 20) must not be \
             contaminated by the provisional probe's price (990)"
        );
    }

    #[test]
    fn a_closed_bar_advances_the_confirmed_state_exactly_once() {
        let mut engine = IndicatorEngine::new(spec(), sma1());
        // Two ticks in the same forming minute-0 bucket: neither is a
        // closed bar, so `on_tick` must never call `advance` for them.
        assert!(
            engine
                .on_tick(&tick(0, 100))
                .is_none_or(|reading| reading.provisional)
        );
        assert!(
            engine
                .on_tick(&tick(30, 110))
                .is_none_or(|reading| reading.provisional)
        );
        // Minute 1 opens, closing minute 0 — exactly one confirmed reading.
        let reading = engine.on_tick(&tick(65, 120)).unwrap();
        assert!(!reading.provisional);
        assert_eq!(reading.values, vec![(IndicatorField::Value, 110.0)]);
    }

    #[test]
    fn rebase_matches_a_fresh_load_over_the_same_history() {
        let history: Vec<_> = (0..20)
            .map(|i| super::tests_support::bar(i * 60, 100 + i))
            .collect();

        let mut incremental = IndicatorEngine::new(spec(), sma1());
        incremental.rebase(&history[..10]);
        // More history is now available to the left — a real "scrolled
        // further back" scenario.
        incremental.rebase(&history);

        let mut fresh = IndicatorEngine::new(spec(), sma1());
        fresh.rebase(&history);

        let incremental_reading = incremental.advance(history.last().unwrap());
        let fresh_reading = fresh.advance(history.last().unwrap());
        assert_eq!(incremental_reading, fresh_reading);
    }
}

#[cfg(test)]
mod tests_support {
    use senken_core::UnixNanos;
    use senken_series::{Bar, Volume};

    pub(crate) fn bar(ts_secs: i64, close: i64) -> Bar {
        Bar {
            ts_open: UnixNanos::from_secs(ts_secs).unwrap(),
            open: close,
            high: close,
            low: close,
            close,
            volume: Volume::Real(0),
            quote_volume: None,
            trade_count: None,
            taker_buy_volume: None,
        }
    }
}

#[cfg(test)]
mod registry_tests {
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;
    use senken_core::UnixNanos;
    use senken_indicators::{ConcreteIndicator, IndicatorField};
    use senken_marketdata::InstrumentId;
    use senken_series::{BarSpec, BarUnit};

    use super::{IndicatorSessionKey, IndicatorSessionRegistry};
    use crate::connection::{ConnectionError, VenueConnection, VenueConnector};
    use crate::pool::SubscriptionPool;

    /// Reproduced rather than shared, exactly as `senken-alerts`' own
    /// runner tests explain: this is private to `senken-subscription`'s own
    /// test module.
    #[derive(Default)]
    struct FakeConnection {
        unsubscribed: Mutex<Vec<InstrumentId>>,
    }

    #[async_trait]
    impl VenueConnection for FakeConnection {
        async fn shutdown(&self) {}

        async fn subscribe(&self, _instrument: &InstrumentId) -> Result<(), ConnectionError> {
            Ok(())
        }

        async fn unsubscribe(&self, instrument: &InstrumentId) -> Result<(), ConnectionError> {
            self.unsubscribed.lock().unwrap().push(instrument.clone());
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
            self.opened.lock().unwrap().push(Arc::clone(&connection));
            Ok(connection)
        }
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new("fake-venue", "BTCUSDT").unwrap()
    }

    fn key() -> IndicatorSessionKey {
        IndicatorSessionKey::new(
            instrument(),
            BarSpec::new(1, BarUnit::Minute),
            "Sma",
            r#"{"period":1}"#,
        )
    }

    fn sma1() -> ConcreteIndicator {
        ConcreteIndicator::build("Sma", r#"{"period":1}"#).unwrap()
    }

    #[tokio::test]
    async fn two_identical_requests_share_one_session_and_it_outlives_the_first_holder() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));
        let registry = IndicatorSessionRegistry::default();

        let first = registry
            .get_or_create(&pool, key(), sma1(), &[])
            .await
            .unwrap();
        let second = registry
            .get_or_create(&pool, key(), sma1(), &[])
            .await
            .unwrap();

        assert_eq!(
            registry.live_sessions().await,
            1,
            "two identical requests must share exactly one session"
        );
        assert_eq!(first.holder_count(), 2);

        drop(first);
        assert_eq!(
            registry.live_sessions().await,
            1,
            "the session must still be live while the second handle holds it"
        );
        assert_eq!(second.holder_count(), 1);

        drop(second);
        assert_eq!(
            registry.live_sessions().await,
            0,
            "the last handle dropping must retire the session"
        );
    }

    #[tokio::test]
    async fn dropping_the_last_handle_releases_the_underlying_lease() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));
        let registry = IndicatorSessionRegistry::default();

        let handle = registry
            .get_or_create(&pool, key(), sma1(), &[])
            .await
            .unwrap();
        let shard = connector.opened.lock().unwrap()[0].clone();
        assert!(shard.unsubscribed.lock().unwrap().is_empty());

        drop(handle);
        pool.flush().await;
        assert_eq!(
            shard.unsubscribed.lock().unwrap().as_slice(),
            &[instrument()],
            "the session's own lease must be released once its last handle drops"
        );
    }

    #[tokio::test]
    async fn a_session_is_warmed_up_once_from_the_supplied_history() {
        let connector = Arc::new(FakeConnector::default());
        let pool = SubscriptionPool::new("fake-venue", Arc::clone(&connector));
        let registry = IndicatorSessionRegistry::default();

        // `Sma(2)` needs exactly two bars to initialize — one supplied as
        // warm-up history, one closed live — so a confirmed reading here
        // can only exist if the session actually replayed the warm-up bar
        // into the canonical indicator rather than starting from nothing.
        let sma2 = ConcreteIndicator::build("Sma", r#"{"period":2}"#).unwrap();
        let warm_up = [super::tests_support::bar(0, 100)];
        let handle = registry
            .get_or_create(
                &pool,
                IndicatorSessionKey::new(
                    instrument(),
                    BarSpec::new(1, BarUnit::Minute),
                    "Sma",
                    r#"{"period":2}"#,
                ),
                sma2,
                &warm_up,
            )
            .await
            .unwrap();
        let mut updates = handle.updates();

        let tick = |secs: i64, price: i64| crate::price::PriceUpdate {
            ts: UnixNanos::from_secs(secs).unwrap(),
            price,
            price_scale: 2,
            qty: senken_series::Volume::Real(1),
            qty_scale: 0,
        };

        // Opens minute-1's bucket — nothing has closed yet, so this can
        // only ever produce a provisional reading.
        pool.publish(instrument(), tick(65, 140));
        // Interleaved with a short sleep, exactly like `senken_alerts`'
        // own runner tests: the watch channel underneath only ever holds
        // the latest tick, so two `publish` calls made back-to-back before
        // anything reads the channel would coalesce and this bucket-opening
        // tick would never be seen at all.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        // Minute 2 opens, closing minute 1 with its close still 140 — now
        // the canonical state has the warm-up bar plus this one, and
        // `Sma(2)` is initialized.
        pool.publish(instrument(), tick(125, 999));

        let reading = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            loop {
                updates.changed().await.unwrap();
                let Some(reading) = updates.borrow_and_update().clone() else {
                    continue;
                };
                if !reading.provisional {
                    return reading;
                }
            }
        })
        .await
        .expect("a confirmed reading must arrive well within this timeout");

        assert_eq!(reading.values, vec![(IndicatorField::Value, 120.0)]);
    }
}
