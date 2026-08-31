//! [`PriorityGate`] — a priority-ordered concurrency gate.
//!
//! Every job on one [`crate::SeriesLoader`] shares one ceiling on concurrent
//! chunk fetches, previously a plain `tokio::sync::Semaphore`
//! (its own report: "the semaphore hands out permits FIFO... this session
//! did not build a cross-job priority queue"). A bare semaphore services
//! whoever asked first, so a `Background` backfill that started earlier can
//! starve a `Visible` chart the user is looking at right now — exactly the
//! gap ("visible range first, then adjacent prefetch, then
//! background backfill") requires closed.
//!
//! This gate keeps the same shape (`acquire` waits for a slot, dropping the
//! returned [`Permit`] frees it) but hands a freed slot to the
//! **highest-[`Priority`] current waiter**, not merely the next one in
//! arrival order. The hand-off is direct — a released slot is given to a
//! specific waiter by name (a `oneshot` send), never returned to a general
//! pool for anyone to race for — so the choice is never a scheduling
//! coincidence: see `priority_gate`'s own tests, and
//! `crate::loader::tests::a_visible_jobs_chunk_is_serviced_before_an_earlier_queued_backgrounds_chunk`,
//! for how this is proven rather than merely trusted (the same standard M6
//! held itself to when it declined to build this unproven).

use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use tokio::sync::oneshot;

use crate::job::Priority;

/// One caller waiting for a slot: ranked by `priority` first, then by
/// arrival order (`seq`) among equal priorities — `Reverse` on both sides so
/// a plain max-`BinaryHeap` pops the highest-`Priority`, earliest-queued
/// waiter first. `grant` is fired exactly once, by [`PriorityGate::release`],
/// to hand this specific waiter the freed slot.
struct Waiter {
    priority: Priority,
    seq: Reverse<u64>,
    grant: oneshot::Sender<()>,
}

impl PartialEq for Waiter {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.seq == other.seq
    }
}

impl Eq for Waiter {}

impl PartialOrd for Waiter {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Waiter {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.priority, self.seq).cmp(&(other.priority, other.seq))
    }
}

struct State {
    /// Free slots not currently claimed by any [`Permit`] or promised to a
    /// waiting [`Waiter`].
    available: usize,
    waiters: BinaryHeap<Waiter>,
}

/// Caps concurrent chunk fetches across every job on one loader,
/// releasing a freed slot to the highest-priority current waiter rather than
/// in arrival order.
pub(crate) struct PriorityGate {
    state: Mutex<State>,
    next_seq: AtomicU64,
}

impl PriorityGate {
    /// Builds a gate with room for `capacity` concurrent [`Permit`]s (at
    /// least one — a gate with none could never let anything proceed).
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(State {
                available: capacity.max(1),
                waiters: BinaryHeap::new(),
            }),
            next_seq: AtomicU64::new(0),
        }
    }

    /// Waits for a slot, ranked by `priority` against every other caller
    /// currently waiting. Returns a [`Permit`] that frees the slot — to the
    /// next highest-priority waiter, if any, or back to the pool — when
    /// dropped.
    pub(crate) async fn acquire(&self, priority: Priority) -> Permit<'_> {
        let pending = {
            let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
            if state.available > 0 {
                state.available -= 1;
                None
            } else {
                let seq = self.next_seq.fetch_add(1, Ordering::SeqCst);
                let (tx, rx) = oneshot::channel();
                state.waiters.push(Waiter {
                    priority,
                    seq: Reverse(seq),
                    grant: tx,
                });
                Some(rx)
            }
        };
        // The lock above is released before awaiting: a waiter is granted
        // its slot by a targeted `oneshot` send from `release`, never by
        // re-checking `available` itself, so there is nothing left to race
        // once this resolves.
        if let Some(pending) = pending {
            let _ = pending.await;
        }
        Permit { gate: self }
    }

    /// How many callers are currently queued for a slot. Used only by this
    /// crate's own tests, to synchronise on a known contention state
    /// without depending on timing.
    #[cfg(test)]
    pub(crate) fn waiting_count(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .waiters
            .len()
    }

    fn release(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            let Some(next) = state.waiters.pop() else {
                state.available += 1;
                return;
            };
            if next.grant.send(()).is_ok() {
                // The slot is now that waiter's — `available` is
                // deliberately not touched, since it was never returned to
                // the general pool.
                return;
            }
            // The waiter gave up (its `acquire` future was dropped) — try
            // the next one instead of leaking the slot it would have
            // received.
        }
    }
}

/// An acquired slot from [`PriorityGate::acquire`]. Frees the slot — to the
/// next highest-priority waiter, or back to the pool — when dropped.
pub(crate) struct Permit<'a> {
    gate: &'a PriorityGate,
}

impl Drop for Permit<'_> {
    fn drop(&mut self) {
        self.gate.release();
    }
}

#[cfg(test)]
mod tests {
    use super::PriorityGate;
    use crate::job::Priority;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    /// Polls `cond` until it is true, yielding between checks rather than
    /// sleeping a guessed duration — deterministic synchronisation on a
    /// known state instead of a timing assumption. Wrapped in a generous
    /// real-time safety net purely so a genuine bug fails the test with a
    /// clear timeout instead of hanging the suite forever; the *outcome*
    /// this test asserts on never depends on wall-clock timing.
    async fn wait_until(mut cond: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if cond() {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("condition was not met before the test's safety timeout");
    }

    /// The core proof this module exists for:
    /// when a slot frees with more than one caller queued, the
    /// highest-`Priority` waiter is granted it, never whichever queued
    /// first. Both waiters are confirmed queued (`waiting_count() == 2`,
    /// polled deterministically) *before* the slot is released, so the
    /// outcome cannot be attributed to scheduling luck — a FIFO queue and
    /// this priority queue would necessarily disagree on this exact
    /// scenario.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_higher_priority_waiter_is_granted_the_slot_ahead_of_an_earlier_lower_priority_one() {
        let gate = Arc::new(PriorityGate::new(1));
        let order = Arc::new(Mutex::new(Vec::new()));

        // Take the sole slot up front so both callers below must queue.
        let held = gate.acquire(Priority::Background).await;

        let g1 = Arc::clone(&gate);
        let o1 = Arc::clone(&order);
        let low = tokio::spawn(async move {
            let _permit = g1.acquire(Priority::Background).await;
            o1.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("background");
        });
        wait_until(|| gate.waiting_count() == 1).await;

        let g2 = Arc::clone(&gate);
        let o2 = Arc::clone(&order);
        let high = tokio::spawn(async move {
            let _permit = g2.acquire(Priority::Visible).await;
            o2.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push("visible");
        });
        wait_until(|| gate.waiting_count() == 2).await;

        // Both waiters are now queued, confirmed above — releasing here is
        // the actual contested decision point this test exists to prove.
        drop(held);

        low.await.unwrap();
        high.await.unwrap();

        assert_eq!(
            *order
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
            vec!["visible", "background"],
            "the Visible waiter, queued second, must still be granted the slot first"
        );
    }

    /// A gate with no contention never blocks — the common case, sanity
    /// checked so the priority machinery above never becomes a hazard when
    /// there is nothing to arbitrate.
    #[tokio::test]
    async fn an_uncontended_gate_grants_immediately_regardless_of_priority() {
        let gate = PriorityGate::new(2);
        let a = gate.acquire(Priority::Background).await;
        let b = gate.acquire(Priority::Visible).await;
        assert_eq!(gate.waiting_count(), 0);
        drop(a);
        drop(b);
    }
}
