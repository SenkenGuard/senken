//! A shared rate/concurrency budget for one venue.
//!
//! [`LimitGroup`] is keyed by **group**, not by source: `binance-spot`,
//! `binance-usdm` and `binance-coinm` are three [`MarketDataSource`]s but one
//! Binance IP, and must draw from one budget or they collectively spend three
//! times the venue's real quota. A plugin creates one group per venue and
//! hands the same (cloned) group to every [`VenueClient`](crate::VenueClient)
//! it builds for that venue's sources.
//!
//! Three independent mechanisms live here, composed by [`LimitGroup::acquire`]:
//!
//! - **Proactive token buckets**, one per [`per_window`](LimitGroup::per_window)
//!   call. Several can be active at once — Binance limits per-minute *and*
//!   per-day — and a request must fit every one of them before it proceeds.
//!   A group with none configured (verified true of OKX's public endpoints)
//!   simply never waits here.
//! - **A concurrency ceiling**, enforced with a [`tokio::sync::Semaphore`] and
//!   adjusted by AIMD: halved on a `429`, grown by one permit after a run of
//!   successes, bounded by whatever [`max_concurrent`](LimitGroup::max_concurrent)
//!   configured.
//! - **A circuit breaker**, tripped by a `418` or by repeated `429`s, that
//!   makes every call fail immediately for a cooldown instead of queueing
//!   behind a ban.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use senken_marketdata::source::SourceError;
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::jitter::full_jitter;

/// No venue's real connection posture is verified here (the capture verifies
/// only response headers, never a documented request limit). This is a
/// deliberately conservative starting ceiling for any group that never calls
/// [`LimitGroup::max_concurrent`] — small enough that even a strict venue is
/// unlikely to reject it outright on the connection level alone.
const DEFAULT_MAX_CONCURRENT: usize = 4;

/// Consecutive `429`s, on top of not yet recovering from the last one, before
/// a group is treated as genuinely banned rather than briefly busy. Our own
/// policy choice, not a venue fact — three survives a single flaky response
/// without giving a real block three free bites at the venue.
const CONSECUTIVE_429_TO_TRIP: u32 = 3;

/// How long a tripped circuit stays open before the next call is allowed to
/// probe the venue again. Conservative and undocumented by any venue on
/// purpose (see the module docs): long enough that an accidental hammering
/// of a cooling-down venue cannot happen, short enough that a spurious trip
/// self-heals inside one interactive session.
const CIRCUIT_COOLDOWN: Duration = Duration::from_secs(30);

/// Consecutive successes required before the concurrency ceiling is grown by
/// one permit (AIMD's additive increase). Kept slow relative to the halving
/// on failure, which is the whole point of AIMD: back off hard, recover slow.
const SUCCESS_STREAK_TO_RESTORE: usize = 5;

/// A shared rate, concurrency and failure budget for one venue.
///
/// Cheap to clone: cloning shares the same [`Arc`]-held state, so every
/// [`VenueClient`](crate::VenueClient) built from clones of one `LimitGroup`
/// draws from the same budget.
#[derive(Clone)]
pub struct LimitGroup {
    inner: Arc<GroupState>,
}

impl std::fmt::Debug for LimitGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LimitGroup")
            .field("name", &self.inner.name)
            .finish_non_exhaustive()
    }
}

impl LimitGroup {
    /// A new group named `name` (used only in logs and fail-fast errors), with
    /// no proactive windows and the conservative default concurrency ceiling.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            inner: Arc::new(GroupState {
                name: name.into(),
                windows: Mutex::new(Vec::new()),
                concurrency: ConcurrencyGate::new(DEFAULT_MAX_CONCURRENT),
                circuit: Circuit::new(),
            }),
        }
    }

    /// Adds a proactive token-bucket window: at most `budget` cost admitted
    /// every `window`. Repeatable — several windows compose, and a request
    /// must fit inside every one of them.
    #[must_use]
    pub fn per_window(self, window: Duration, budget: u32) -> Self {
        self.inner
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(WindowState {
                duration: window,
                budget,
                window_start: Instant::now(),
                used: 0,
            });
        self
    }

    /// Sets (or resets) the concurrency ceiling. Takes effect immediately:
    /// growing adds permits right away, shrinking reclaims them as soon as
    /// they are free.
    #[must_use]
    pub fn max_concurrent(self, n: usize) -> Self {
        self.inner.concurrency.reconfigure(n);
        self
    }

    /// The name this group was created with.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// Waits for capacity — proactive windows, then the concurrency ceiling —
    /// and returns a permit that must be held for the lifetime of the
    /// request. Fails immediately, without waiting for anything, if the
    /// circuit is currently open.
    pub(crate) async fn acquire(&self, cost: u32) -> Result<GroupPermit, SourceError> {
        self.inner.circuit.ensure_closed(&self.inner.name)?;
        self.inner.acquire_windows(cost).await;
        let permit = self.inner.concurrency.acquire().await;
        // The wait above can be long; re-check rather than admit a request
        // the venue has since banned us for.
        self.inner.circuit.ensure_closed(&self.inner.name)?;
        Ok(GroupPermit {
            _concurrency: permit,
        })
    }

    /// Reconciles a proactive window to the venue's own accounting — its
    /// count is authoritative, ours is a guess.
    pub(crate) fn reconcile_window(&self, window: Duration, observed_used: u32) {
        let mut windows = self
            .inner
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for state in windows.iter_mut().filter(|w| w.duration == window) {
            state.used = observed_used;
        }
    }

    /// A `2xx` completed: closes the failure streak and, after enough of
    /// these in a row, grows the concurrency ceiling by one.
    pub(crate) fn record_success(&self) {
        self.inner.circuit.record_success();
        self.inner.concurrency.record_success();
    }

    /// A `429` completed. Halves the concurrency ceiling unconditionally
    /// (AIMD) and returns `true` if this was the one that tripped the
    /// circuit breaker.
    pub(crate) fn record_429(&self) -> bool {
        self.inner.concurrency.halve();
        self.inner.circuit.record_429()
    }

    /// A `418` completed: trip the circuit immediately, no threshold.
    pub(crate) fn trip_circuit(&self) {
        self.inner.circuit.trip();
    }

    /// Waits for this group's proactive-window and concurrency budget the
    /// same way [`VenueClient::get`](crate::VenueClient::get) does, for a
    /// caller whose request is not an HTTP fetch at all.
    ///
    /// This exists because a venue's live-price WebSocket
    /// shares the same IP-level budget as its REST catalog/kline traffic
    /// (`binance-spot`, `binance-usdm` and `binance-coinm` are three sources
    /// but one Binance IP — the same fact M3.1 states for HTTP applies to a
    /// socket dial), so opening one through a side channel that never
    /// touches this group would spend quota the group does not know about.
    /// The returned [`ConnectPermit`] must be held for the duration of the
    /// dial and dropped once it completes (success or failure) — exactly
    /// how `VenueClient::get` holds its own permit across one HTTP round
    /// trip.
    ///
    /// Deliberately narrower than this group's internal `acquire`: it does not
    /// feed a dial's outcome into the `429`/`418` circuit breaker — those
    /// are HTTP status semantics a WS handshake failure does not have, and
    /// inventing a mapping between them would be exactly the kind of
    /// unverified per-venue behaviour. A dial still
    /// fails fast with [`SourceError::Rejected`] if the group's circuit is
    /// already open from HTTP traffic, since that *is* a fact the group
    /// already knows.
    ///
    /// # Errors
    /// A [`SourceError::Rejected`] if the circuit is currently open.
    pub async fn acquire_for_connect(&self, cost: u32) -> Result<ConnectPermit, SourceError> {
        let permit = self.acquire(cost).await?;
        Ok(ConnectPermit { _permit: permit })
    }
}

/// Held for the duration of one non-HTTP dial gated by
/// [`LimitGroup::acquire_for_connect`]. Dropping it returns the concurrency
/// permit to the group, exactly like the private `GroupPermit` an HTTP
/// request holds internally.
#[derive(Debug)]
#[must_use = "dropping this immediately releases the connect slot"]
pub struct ConnectPermit {
    _permit: GroupPermit,
}

/// Held for the lifetime of one request. Dropping it returns the concurrency
/// permit to the group.
#[derive(Debug)]
pub(crate) struct GroupPermit {
    _concurrency: OwnedSemaphorePermit,
}

struct GroupState {
    name: Box<str>,
    windows: Mutex<Vec<WindowState>>,
    concurrency: ConcurrencyGate,
    circuit: Circuit,
}

impl GroupState {
    /// Blocks until every configured window has room for `cost`. A group
    /// with no windows returns immediately — this is the pure-concurrency
    /// path a venue with no documented quota (or no headers to react to,
    /// like OKX) still needs.
    async fn acquire_windows(&self, cost: u32) {
        loop {
            match self.try_reserve_all(cost) {
                None => return,
                Some(wait) => {
                    tokio::time::sleep(full_jitter(wait.max(Duration::from_millis(1)))).await;
                }
            }
        }
    }

    /// Either reserves `cost` against every window at once, or reserves
    /// nothing and reports how long the slowest window needs. Never a
    /// partial reservation — that would let a request "spend" quota on one
    /// window while still waiting on another, which double counts on retry.
    fn try_reserve_all(&self, cost: u32) -> Option<Duration> {
        let mut windows = self
            .windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if windows.is_empty() {
            return None;
        }

        let now = Instant::now();
        for window in windows.iter_mut() {
            if now.duration_since(window.window_start) >= window.duration {
                window.window_start = now;
                window.used = 0;
            }
        }

        // A request heavier than the whole window can never fit; let it
        // through once rather than waiting forever on a config mistake.
        let ready = |w: &WindowState| cost > w.budget || w.used.saturating_add(cost) <= w.budget;

        if windows.iter().all(ready) {
            for window in windows.iter_mut() {
                window.used = window.used.saturating_add(cost).min(window.budget);
            }
            None
        } else {
            windows
                .iter()
                .filter(|w| !ready(w))
                .map(|w| (w.window_start + w.duration).saturating_duration_since(now))
                .max()
        }
    }
}

/// One `per_window` bucket's live state.
struct WindowState {
    duration: Duration,
    budget: u32,
    window_start: Instant,
    used: u32,
}

/// The concurrency ceiling, adjusted by AIMD.
///
/// Built on one [`Semaphore`] sized to the *configured* maximum. Halving
/// reclaims permits with [`Semaphore::forget_permits`] so they are gone from
/// the pool rather than merely held; restoring adds them back with
/// [`Semaphore::add_permits`]. Both are the documented way to resize a
/// `tokio` semaphore at runtime.
struct ConcurrencyGate {
    semaphore: Arc<Semaphore>,
    configured_max: AtomicUsize,
    /// Permits currently in the pool (available + checked out). Always
    /// `<= configured_max`.
    current_capacity: AtomicUsize,
    success_streak: AtomicUsize,
}

impl ConcurrencyGate {
    fn new(max_concurrent: usize) -> Self {
        let max_concurrent = max_concurrent.max(1);
        Self {
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            configured_max: AtomicUsize::new(max_concurrent),
            current_capacity: AtomicUsize::new(max_concurrent),
            success_streak: AtomicUsize::new(0),
        }
    }

    async fn acquire(&self) -> OwnedSemaphorePermit {
        Arc::clone(&self.semaphore)
            .acquire_owned()
            .await
            .expect("limit group semaphore is never closed")
    }

    /// Changes the configured ceiling, growing or shrinking the live pool to
    /// match immediately.
    fn reconfigure(&self, new_max: usize) {
        let new_max = new_max.max(1);
        self.configured_max.store(new_max, Ordering::Release);
        let current = self.current_capacity.load(Ordering::Acquire);
        match new_max.cmp(&current) {
            std::cmp::Ordering::Greater => {
                self.semaphore.add_permits(new_max - current);
                self.current_capacity.store(new_max, Ordering::Release);
            }
            std::cmp::Ordering::Less => {
                self.shrink_to(new_max);
            }
            std::cmp::Ordering::Equal => {}
        }
    }

    /// Best-effort shrink: [`Semaphore::forget_permits`] reclaims whatever is
    /// currently free and reports how much it actually managed. If every
    /// permit is checked out right now this reclaims nothing, and a later
    /// halve or reconfigure converges it — this only matters under
    /// concurrent load, and AIMD is inherently a converging approximation,
    /// not an instantaneous guarantee.
    fn shrink_to(&self, target: usize) {
        let current = self.current_capacity.load(Ordering::Acquire);
        if target >= current {
            return;
        }
        let forgotten = self.semaphore.forget_permits(current - target);
        self.current_capacity
            .store(current - forgotten, Ordering::Release);
    }

    /// AIMD multiplicative decrease.
    fn halve(&self) {
        self.success_streak.store(0, Ordering::Release);
        let current = self.current_capacity.load(Ordering::Acquire);
        self.shrink_to((current / 2).max(1));
    }

    /// AIMD additive increase, after a sustained run of successes.
    fn record_success(&self) {
        let streak = self.success_streak.fetch_add(1, Ordering::AcqRel) + 1;
        if streak < SUCCESS_STREAK_TO_RESTORE {
            return;
        }
        self.success_streak.store(0, Ordering::Release);
        let max = self.configured_max.load(Ordering::Acquire);
        let current = self.current_capacity.load(Ordering::Acquire);
        if current < max {
            self.semaphore.add_permits(1);
            self.current_capacity.store(current + 1, Ordering::Release);
        }
    }
}

/// Whether requests are currently allowed at all.
enum CircuitState {
    Closed { consecutive_429: u32 },
    Open { until: Instant },
}

/// Fails fast for a cooldown after a `418` or a run of `429`s, instead of
/// letting every in-flight and queued request individually discover the ban.
struct Circuit {
    state: Mutex<CircuitState>,
}

impl Circuit {
    fn new() -> Self {
        Self {
            state: Mutex::new(CircuitState::Closed { consecutive_429: 0 }),
        }
    }

    fn ensure_closed(&self, group: &str) -> Result<(), SourceError> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let CircuitState::Open { until } = *state {
            if Instant::now() < until {
                return Err(SourceError::rejected(format!(
                    "{group}: circuit open after repeated 429/418, cooling down"
                )));
            }
            // Cooldown elapsed — close it and let this request probe the
            // venue rather than waiting for an explicit reset.
            *state = CircuitState::Closed { consecutive_429: 0 };
        }
        Ok(())
    }

    fn trip(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *state = CircuitState::Open {
            until: Instant::now() + CIRCUIT_COOLDOWN,
        };
    }

    /// Returns `true` when this call is what tripped the breaker.
    fn record_429(&self) -> bool {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match &mut *state {
            CircuitState::Closed { consecutive_429 } => {
                *consecutive_429 += 1;
                if *consecutive_429 >= CONSECUTIVE_429_TO_TRIP {
                    *state = CircuitState::Open {
                        until: Instant::now() + CIRCUIT_COOLDOWN,
                    };
                    true
                } else {
                    false
                }
            }
            CircuitState::Open { .. } => true,
        }
    }

    fn record_success(&self) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let CircuitState::Closed { consecutive_429 } = &mut *state {
            *consecutive_429 = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LimitGroup;
    use std::time::Duration;

    #[tokio::test]
    async fn a_group_with_no_windows_never_waits_on_quota() {
        // The OKX case: no headers, and here, no configured
        // window either. Only the concurrency ceiling should gate it.
        let group = LimitGroup::new("no-window");
        for _ in 0..100 {
            group.acquire(1).await.unwrap();
        }
    }

    #[tokio::test]
    async fn a_window_budget_is_shared_by_every_caller_of_the_group() {
        let group = LimitGroup::new("shared").per_window(Duration::from_mins(1), 2);
        // Two calls exhaust the budget of two — from the same group, so it
        // does not matter that they look like two different callers.
        group.acquire(1).await.unwrap();
        group.acquire(1).await.unwrap();

        // A third call over budget must wait — proven by racing it against a
        // timeout rather than sleeping a fixed amount in the test.
        let waited = tokio::time::timeout(Duration::from_millis(50), group.acquire(1)).await;
        assert!(
            waited.is_err(),
            "a third unit over a budget of two must not be admitted immediately"
        );
    }

    #[tokio::test]
    async fn a_request_heavier_than_the_whole_window_is_let_through_once() {
        // A misconfigured cost must not deadlock the caller forever.
        let group = LimitGroup::new("oversized").per_window(Duration::from_mins(1), 1);
        let result = tokio::time::timeout(Duration::from_millis(50), group.acquire(100)).await;
        assert!(
            result.is_ok(),
            "an over-budget single request must not hang"
        );
    }

    #[tokio::test]
    async fn reconciling_a_window_adopts_the_venues_own_count() {
        let group = LimitGroup::new("reconciled").per_window(Duration::from_mins(1), 10);
        group.acquire(1).await.unwrap();
        // The venue says we are already at 10/10 — trust it over our own
        // count of 1.
        group.reconcile_window(Duration::from_mins(1), 10);
        let waited = tokio::time::timeout(Duration::from_millis(50), group.acquire(1)).await;
        assert!(
            waited.is_err(),
            "reconciliation must make the bucket reflect the venue's count, not ours"
        );
    }

    #[tokio::test]
    async fn a_418_trips_the_circuit_and_the_next_call_fails_fast() {
        let group = LimitGroup::new("banned");
        group.trip_circuit();

        let start = std::time::Instant::now();
        let error = group.acquire(1).await.unwrap_err();
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "a tripped circuit must fail fast, not queue behind the cooldown"
        );
        assert!(
            !error.is_retryable(),
            "a circuit-open rejection is not itself retryable"
        );
    }

    #[tokio::test]
    async fn repeated_429s_trip_the_circuit_even_without_a_418() {
        let group = LimitGroup::new("flaky");
        assert!(!group.record_429());
        assert!(!group.record_429());
        assert!(group.record_429(), "the third consecutive 429 must trip it");
        assert!(group.acquire(1).await.is_err());
    }

    #[tokio::test]
    async fn a_non_http_dial_shares_the_same_concurrency_ceiling_as_http_requests() {
        // a venue's live-price WS dial must draw on the same
        // budget as its REST traffic, not an unbudgeted side channel.
        let group = LimitGroup::new("shared-dial").max_concurrent(1);
        let http_permit = group.acquire(1).await.unwrap();

        let waited =
            tokio::time::timeout(Duration::from_millis(50), group.acquire_for_connect(1)).await;
        assert!(
            waited.is_err(),
            "a dial must wait behind an HTTP request already holding the group's only slot"
        );

        drop(http_permit);
        let dial_permit = group.acquire_for_connect(1).await.unwrap();
        drop(dial_permit);
    }

    #[tokio::test]
    async fn a_dial_still_fails_fast_when_the_circuit_is_already_open() {
        let group = LimitGroup::new("dial-vs-circuit");
        group.trip_circuit();

        let start = std::time::Instant::now();
        let error = group.acquire_for_connect(1).await.unwrap_err();
        assert!(
            start.elapsed() < Duration::from_millis(50),
            "a dial must fail fast rather than queue behind a cooldown the group already knows about"
        );
        assert!(!error.is_retryable());
    }

    #[tokio::test]
    async fn a_success_resets_the_429_streak() {
        let group = LimitGroup::new("recovering");
        assert!(!group.record_429());
        assert!(!group.record_429());
        group.record_success();
        // The streak was reset, so two more 429s alone must not trip it.
        assert!(!group.record_429());
        assert!(!group.record_429());
    }
}
