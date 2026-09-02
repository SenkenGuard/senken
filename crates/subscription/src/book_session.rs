//! A fixed-depth order book kept current: one poll loop per instrument,
//! shared by everything watching it.
//!
//! [`BookSource`] is a request/response port — one call, one snapshot, no
//! stream, for the reasons [`senken_marketdata::book`] documents. That is the right
//! shape for the venue call and the wrong shape for a reader: depth that is
//! only as fresh as the moment a panel opened is not depth, and asking the
//! reader to press a button for the next one makes staying current their
//! job rather than the platform's.
//!
//! This module closes that gap without pretending the port is something it
//! is not. It asks again, on a cadence, and republishes each answer on a
//! [`watch`] channel — the same shape [`crate::IndicatorSessionRegistry`]
//! already uses for a live indicator, and for the same reason: the work is
//! done once no matter how many consumers want the result. Two panes on
//! `okx-spot:BTC-USDT` share one poll loop and cost the venue one request
//! per interval between them, not two.
//!
//! Nothing here merges snapshots or tracks a sequence number across them.
//! Each published state is one venue-reported instant, whole — a locally
//! maintained book built from deltas is still the complexity
//! [`crate::BookSnapshot`]'s own docs decline to take on.

use std::collections::HashMap;
use std::sync::{Arc, Weak};
use std::time::Duration;

use senken_marketdata::{InstrumentId, SourceSymbol};
use tokio::sync::{Mutex, watch};
use tokio::task::AbortHandle;

use senken_marketdata::book::{BookSnapshot, BookSource};

/// How often a live session asks its venue for a fresh snapshot.
///
/// This project's own cadence for a depth panel, not a venue-documented
/// figure and not derived from one: no venue this build talks to reports a
/// rate-limit weight for its book endpoint in a response header (see
/// `senken-feed`'s `okx_book` module docs). It is fast enough that a ladder
/// reads as live to a person watching it, and slow enough that several open
/// panels stay well inside the deliberately conservative proactive budget
/// their `LimitGroup` is given. Treat it as a budget, not a measurement.
pub const DEFAULT_REFRESH_INTERVAL: Duration = Duration::from_secs(1);

/// Consecutive failed polls a session that has already served a snapshot
/// tolerates before it reports [`BookState::Failed`].
///
/// One dropped request is not a broken book, and blanking a working ladder
/// on a single blip puts an error in front of a reader for a second and
/// then takes it away again. Three in a row is not a blip — and past that
/// point, continuing to show the last snapshot would be presenting stale
/// depth as live, which is the one failure mode a book panel must not have.
/// A session that has *never* answered does not wait for three: there is
/// nothing on screen to protect, and "still loading" would be a lie.
const FAILURES_BEFORE_STALE: u32 = 3;

/// What a live book session currently knows.
///
/// Three states, never collapsed: not asked yet, a real snapshot, and
/// asked-and-could-not-get-one. A consumer that cannot tell the first from
/// the third shows "loading" forever for a book that is never coming.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BookState {
    /// The venue has not answered this session's first request yet.
    Pending,
    /// The most recent snapshot the venue reported, whole.
    ///
    /// Shared rather than cloned per consumer: a twenty-level ladder is
    /// republished every interval to every watcher, and the snapshot is
    /// immutable once built.
    Live(Arc<BookSnapshot>),
    /// Depth could not be fetched, and what was last reported (if anything)
    /// is no longer to be trusted as current.
    ///
    /// A session that has already served a snapshot does not report this on
    /// the first miss — it tolerates a short run of them first, so a single
    /// dropped request does not blank a working ladder. A session that has
    /// never answered reports it at once: there is nothing on screen to
    /// protect, and "still loading" would be a lie.
    Failed,
}

/// The shared record behind every handle onto one instrument's live book.
struct BookSessionInner {
    instrument: InstrumentId,
    updates: watch::Sender<BookState>,
    task: AbortHandle,
}

impl Drop for BookSessionInner {
    fn drop(&mut self) {
        tracing::debug!(
            instrument = %self.instrument,
            "retiring a live book session — its last holder just dropped"
        );
        // Stops the poll loop. Nothing else has to happen: this session
        // holds no venue connection, only a repeating HTTP request that
        // must not outlive the last consumer of its answers.
        self.task.abort();
    }
}

/// A reference-counted handle onto one instrument's live book session.
///
/// Two panels showing depth for the same instrument hold two handles onto
/// the *same* session, exactly as two leaseholders on one instrument share
/// one [`crate::SubscriptionPool`] watch channel. The poll loop stops only
/// once every handle onto it has been dropped; there is deliberately no
/// `close` method, matching [`crate::Lease`]'s own `Drop`-only shape.
#[must_use = "dropping this immediately releases this handle's share of the session"]
pub struct BookSessionHandle {
    inner: Arc<BookSessionInner>,
}

impl BookSessionHandle {
    /// The instrument this session polls.
    #[must_use]
    pub fn instrument(&self) -> &InstrumentId {
        &self.inner.instrument
    }

    /// A receiver for this session's states. Mints an independent receiver
    /// starting from the channel's *current* value, so a consumer joining a
    /// session that is already running sees the snapshot it already has
    /// rather than waiting a whole interval for the next one.
    #[must_use]
    pub fn updates(&self) -> watch::Receiver<BookState> {
        self.inner.updates.subscribe()
    }

    /// How many handles (including this one) currently share the underlying
    /// session — read off the `Arc` doing the reference counting rather
    /// than a second counter that could drift from it.
    #[must_use]
    pub fn holder_count(&self) -> usize {
        Arc::strong_count(&self.inner)
    }
}

impl std::fmt::Debug for BookSessionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BookSessionHandle")
            .field("instrument", &self.inner.instrument)
            .field("holders", &self.holder_count())
            .finish_non_exhaustive()
    }
}

/// Deduplicates live book sessions by instrument, reference-counted and
/// released on `Drop`.
///
/// One registry is expected per server, the same way one
/// [`crate::SubscriptionPool`] is built per venue.
///
/// Depth belongs to the registry, not to the caller asking for a session,
/// and that is deliberate: consumers *share* a session, so a per-call depth
/// would mean the second caller silently getting whatever depth the first
/// one happened to ask for. One setting for the whole registry makes that
/// disagreement impossible to express.
pub struct BookSessionRegistry {
    depth: usize,
    interval: Duration,
    sessions: Mutex<HashMap<InstrumentId, Weak<BookSessionInner>>>,
}

impl BookSessionRegistry {
    /// A registry whose sessions fetch `depth` levels per side, refreshing
    /// every [`DEFAULT_REFRESH_INTERVAL`].
    #[must_use]
    pub fn new(depth: usize) -> Self {
        Self {
            depth,
            interval: DEFAULT_REFRESH_INTERVAL,
            sessions: Mutex::new(HashMap::new()),
        }
    }

    /// The same registry, refreshing every `interval` instead.
    #[must_use]
    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.interval = interval;
        self
    }

    /// How often this registry's sessions refresh.
    #[must_use]
    pub fn interval(&self) -> Duration {
        self.interval
    }

    /// A handle onto the live book for `instrument`, starting a poll loop
    /// against `source`/`symbol` if none is currently running.
    ///
    /// `source` and `symbol` are read only when a session is actually
    /// created: a second caller within a session's lifetime joins the
    /// existing loop, whatever it passes. The lock is held across creating
    /// the session so two concurrent callers for the same brand-new
    /// instrument cannot both start one — the same "one at a time"
    /// discipline [`crate::SubscriptionPool`]'s actor gives lease creation.
    pub async fn get_or_create(
        &self,
        instrument: InstrumentId,
        source: Arc<dyn BookSource>,
        symbol: SourceSymbol,
    ) -> BookSessionHandle {
        let mut sessions = self.sessions.lock().await;
        if let Some(inner) = sessions.get(&instrument).and_then(Weak::upgrade) {
            tracing::debug!(
                instrument = %instrument,
                holders = Arc::strong_count(&inner) + 1,
                "joining an existing live book session"
            );
            return BookSessionHandle { inner };
        }

        let (tx, _) = watch::channel(BookState::Pending);
        let task = tokio::spawn(drive(source, symbol, self.depth, self.interval, tx.clone()));

        let inner = Arc::new(BookSessionInner {
            instrument: instrument.clone(),
            updates: tx,
            task: task.abort_handle(),
        });
        sessions.insert(instrument.clone(), Arc::downgrade(&inner));
        sessions.retain(|_, weak| weak.strong_count() > 0);
        tracing::debug!(
            instrument = %instrument,
            live_sessions = sessions.len(),
            "opened a new live book session"
        );
        BookSessionHandle { inner }
    }

    /// How many sessions are currently live. Also prunes entries whose last
    /// handle has already been dropped, so this never overcounts a session
    /// nothing holds any more.
    pub async fn live_sessions(&self) -> usize {
        let mut sessions = self.sessions.lock().await;
        sessions.retain(|_, weak| weak.strong_count() > 0);
        sessions.len()
    }
}

/// Polls `source` for `symbol` forever, republishing each answer.
///
/// Send failures are ignored rather than ending the loop: the sender lives
/// in the session record, so a moment with no receiver (every consumer
/// between one subscribe and the next) is ordinary, not a reason to stop
/// polling. The loop ends exactly one way — the last handle drops and
/// [`BookSessionInner::drop`] aborts it.
async fn drive(
    source: Arc<dyn BookSource>,
    symbol: SourceSymbol,
    depth: usize,
    interval: Duration,
    updates: watch::Sender<BookState>,
) {
    let mut failures: u32 = 0;
    loop {
        match source.book_snapshot(&symbol, depth).await {
            Ok(snapshot) => {
                failures = 0;
                let _ = updates.send(BookState::Live(Arc::new(snapshot)));
            }
            Err(error) => {
                failures = failures.saturating_add(1);
                let answered = !matches!(*updates.borrow(), BookState::Pending);
                tracing::warn!(
                    %error,
                    symbol = symbol.as_str(),
                    failures,
                    "a live book snapshot poll failed"
                );
                if !answered || failures >= FAILURES_BEFORE_STALE {
                    // Only on the transition. Re-sending `Failed` every
                    // interval would wake every consumer to tell it nothing
                    // it did not already know.
                    updates.send_if_modified(|state| {
                        if *state == BookState::Failed {
                            return false;
                        }
                        *state = BookState::Failed;
                        true
                    });
                }
            }
        }
        tokio::time::sleep(interval).await;
    }
}

#[cfg(test)]
mod tests {
    use super::{BookSessionRegistry, BookState};
    use senken_core::UnixNanos;
    use senken_marketdata::book::{BookLevel, BookSnapshot, BookSource};
    use senken_marketdata::source::SourceError;
    use senken_marketdata::{InstrumentId, SourceSymbol};
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::time::Duration;
    use tokio::sync::watch;

    /// Short enough that a test's waits are cheap, long enough that a poll
    /// and its `changed()` observation are not fighting the scheduler.
    const TICK: Duration = Duration::from_millis(5);

    /// A venue that answers from a script and counts how many times it was
    /// asked. The count is published on a `watch` channel so a test can
    /// *wait for the call to have happened* rather than sleep towards it —
    /// the difference between establishing a precondition and hoping for
    /// one.
    struct ScriptedBook {
        calls: watch::Sender<usize>,
        answers: Mutex<Vec<Result<BookSnapshot, SourceError>>>,
        depths: Mutex<Vec<usize>>,
    }

    fn snapshot(price: i64) -> BookSnapshot {
        BookSnapshot::new(
            UnixNanos::EPOCH,
            vec![BookLevel { price, size: 1 }],
            1,
            1,
            vec![BookLevel {
                price: price + 1,
                size: 1,
            }],
            1,
            1,
        )
        .expect("scales match")
    }

    impl ScriptedBook {
        /// Answers `answers` in order; once exhausted, repeats the last one
        /// forever — a poll loop asks indefinitely, and a finite script
        /// would otherwise decide the test's outcome by running out.
        fn new(answers: Vec<Result<BookSnapshot, SourceError>>) -> Arc<Self> {
            let (calls, _) = watch::channel(0);
            Arc::new(Self {
                calls,
                answers: Mutex::new(answers),
                depths: Mutex::new(Vec::new()),
            })
        }

        fn call_count(&self) -> usize {
            *self.calls.borrow()
        }

        /// Resolves once the venue has been asked at least `n` times.
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
    impl BookSource for ScriptedBook {
        fn source_id(&self) -> &'static str {
            "okx-spot"
        }

        async fn book_snapshot(
            &self,
            _symbol: &SourceSymbol,
            depth: usize,
        ) -> Result<BookSnapshot, SourceError> {
            self.depths.lock().expect("not poisoned").push(depth);
            let answer = {
                let mut answers = self.answers.lock().expect("not poisoned");
                if answers.len() > 1 {
                    answers.remove(0)
                } else {
                    match answers.first() {
                        Some(Ok(snapshot)) => Ok(snapshot.clone()),
                        Some(Err(_)) | None => Err(SourceError::rejected("scripted failure")),
                    }
                }
            };
            self.calls.send_modify(|count| *count += 1);
            answer
        }
    }

    fn registry() -> BookSessionRegistry {
        BookSessionRegistry::new(3).with_interval(TICK)
    }

    fn instrument() -> InstrumentId {
        InstrumentId::parse("okx-spot:BTC-USDT").expect("a well-formed id")
    }

    fn symbol() -> SourceSymbol {
        SourceSymbol::assume("BTC-USDT")
    }

    #[tokio::test]
    async fn a_session_keeps_publishing_fresh_snapshots_without_being_asked_again() {
        // The whole point: a consumer subscribes once and the book stays
        // current. If this only ever published one snapshot, depth would be
        // as stale as the moment the panel opened.
        let source = ScriptedBook::new(vec![
            Ok(snapshot(100)),
            Ok(snapshot(200)),
            Ok(snapshot(300)),
        ]);
        let registry = registry();
        let handle = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;
        let mut updates = handle.updates();

        let mut seen = Vec::new();
        while seen.len() < 3 {
            updates.changed().await.expect("the session is alive");
            if let BookState::Live(snapshot) = &*updates.borrow_and_update() {
                seen.push(snapshot.bids[0].price);
            }
        }

        assert_eq!(seen, vec![100, 200, 300]);
    }

    #[tokio::test]
    async fn two_consumers_of_one_instrument_share_one_poll_loop() {
        // Two panes on the same instrument must cost the venue one request
        // per interval, not one each. A registry that keyed on the caller
        // instead of the instrument would double the venue traffic for a
        // duplicate view.
        let source = ScriptedBook::new(vec![Ok(snapshot(100))]);
        let registry = registry();
        let first = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;
        let second = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;

        assert_eq!(first.holder_count(), 2);
        assert_eq!(second.holder_count(), 2);
        assert_eq!(registry.live_sessions().await, 1);

        source.asked_at_least(4).await;
        // Four polls' worth of calls, from one loop — not eight.
        assert!(
            source.call_count() < 8,
            "two handles started two poll loops: {} calls",
            source.call_count()
        );
    }

    #[tokio::test]
    async fn a_consumer_joining_a_running_session_sees_its_current_snapshot_at_once() {
        // Not after a whole interval. A late joiner that started at
        // `Pending` would show "waiting for depth" next to a pane already
        // rendering the same book.
        let source = ScriptedBook::new(vec![Ok(snapshot(100))]);
        let registry = registry();
        let first = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;
        let mut updates = first.updates();
        updates.changed().await.expect("the session is alive");

        let second = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;

        assert!(matches!(*second.updates().borrow(), BookState::Live(_)));
    }

    #[tokio::test]
    async fn the_last_handle_dropping_stops_the_venue_traffic() {
        // A poll loop that outlived its consumers would keep asking the
        // venue for a book nothing is showing, forever.
        let source = ScriptedBook::new(vec![Ok(snapshot(100))]);
        let registry = registry();
        let handle = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;
        source.asked_at_least(2).await;
        drop(handle);

        // The abort has already run — `Drop` is synchronous and the loop
        // cannot be polled again — so this window measures whether polling
        // stopped, not whether it has got round to stopping.
        let after_drop = source.call_count();
        tokio::time::sleep(TICK * 10).await;

        assert_eq!(source.call_count(), after_drop);
        assert_eq!(registry.live_sessions().await, 0);
    }

    #[tokio::test]
    async fn a_book_that_never_arrives_is_reported_as_failed_immediately() {
        // Nothing is on screen to protect, so there is no reason to wait:
        // "still loading" for a book that is never coming is a lie the
        // consumer cannot see through.
        let source = ScriptedBook::new(vec![Err(SourceError::http(500, "down"))]);
        let registry = registry();
        let handle = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;
        let mut updates = handle.updates();
        updates.changed().await.expect("the session is alive");

        assert_eq!(*updates.borrow(), BookState::Failed);
    }

    #[tokio::test]
    async fn one_dropped_poll_does_not_blank_a_working_ladder() {
        // A single blip must not replace a good book with an error and then
        // take it away again a moment later.
        let source = ScriptedBook::new(vec![
            Ok(snapshot(100)),
            Err(SourceError::http(502, "blip")),
            Ok(snapshot(300)),
        ]);
        let registry = registry();
        let handle = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;
        let updates = handle.updates();

        // Wait until the failing poll has actually happened — the state
        // this assertion is about must exist before it is worth anything.
        source.asked_at_least(2).await;
        assert!(matches!(*updates.borrow(), BookState::Live(_)));
    }

    #[tokio::test]
    async fn a_book_that_stops_refreshing_stops_being_shown() {
        // Past a few consecutive failures the last snapshot is no longer
        // current, and showing it unmarked would be presenting stale depth
        // as live. `FAILURES_BEFORE_STALE` is what decides "a few".
        let source =
            ScriptedBook::new(vec![Ok(snapshot(100)), Err(SourceError::http(502, "gone"))]);
        let registry = registry();
        let handle = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;
        let mut updates = handle.updates();
        updates.changed().await.expect("the session is alive");
        assert!(matches!(*updates.borrow_and_update(), BookState::Live(_)));

        updates.changed().await.expect("the session is alive");
        assert_eq!(*updates.borrow(), BookState::Failed);
        assert!(
            source.call_count() > super::FAILURES_BEFORE_STALE as usize,
            "gave up after {} calls, before the tolerated run of failures",
            source.call_count()
        );
    }

    #[tokio::test]
    async fn a_venue_that_recovers_puts_the_book_back_without_anyone_retrying() {
        // The loop keeps asking, so a failure heals itself — the retry
        // control a consumer offers is a shortcut, never the only way back.
        let source = ScriptedBook::new(vec![
            Err(SourceError::http(500, "down")),
            Err(SourceError::http(500, "down")),
            Ok(snapshot(777)),
        ]);
        let registry = registry();
        let handle = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;
        let mut updates = handle.updates();

        loop {
            updates.changed().await.expect("the session is alive");
            if let BookState::Live(snapshot) = &*updates.borrow_and_update() {
                assert_eq!(snapshot.bids[0].price, 777);
                break;
            }
        }
    }

    #[tokio::test]
    async fn every_poll_asks_for_the_registry_s_own_depth() {
        // Depth is the registry's, not the caller's, precisely so two
        // consumers sharing a session cannot disagree about it.
        let source = ScriptedBook::new(vec![Ok(snapshot(100))]);
        let registry = BookSessionRegistry::new(7).with_interval(TICK);
        let handle = registry
            .get_or_create(
                instrument(),
                Arc::clone(&source) as Arc<dyn BookSource>,
                symbol(),
            )
            .await;
        source.asked_at_least(2).await;
        drop(handle);

        let depths = source.depths.lock().expect("not poisoned").clone();
        assert!(depths.iter().all(|&d| d == 7), "asked for {depths:?}");
    }
}
