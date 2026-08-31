//! Full jitter for retry and rate-limit backoff.
//!
//! A computed backoff duration is never slept as-is: [`full_jitter`] instead
//! picks uniformly from `[0, base]` (Marc Brooker / AWS,
//! *Exponential Backoff and Jitter*). Capping or adding a small random
//! fraction still lets many callers converge on the same wall-clock instant;
//! sampling the whole range is what actually spreads them out, because it can
//! also choose a very short wait.
//!
//! This matters concretely here: 50 sources sharing one 24-hour catalog TTL
//! expire together and refetch at startup. Without jitter, any of them that
//! hit a retryable error back off in lockstep and hit the venue again in
//! lockstep.
//!
//! No RNG crate is pulled in for this — jitter only needs unpredictability
//! across concurrent callers, not cryptographic quality or reproducibility,
//! and the project already avoids a dependency (`governor`, see the plan's
//! rather than take one for a narrow need. A tiny
//! self-seeded xorshift generator, local to this module, is enough.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// One extra source of entropy per thread so two threads seeded in the same
// nanosecond (very possible right after startup, exactly the thundering-herd
// case this exists to break) still diverge.
static SEED_COUNTER: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static STATE: Cell<u64> = Cell::new(seed());
}

fn seed() -> u64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0x9E37_79B9_7F4A_7C15, |d| {
            u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
        });
    let salt = SEED_COUNTER.fetch_add(1, Ordering::Relaxed);
    // xorshift's state must never be zero, or it stays zero forever.
    (nanos ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)).max(1)
}

/// One step of `xorshift64*`. Not suitable for anything security-sensitive;
/// good enough for spreading out retries.
fn next_u64() -> u64 {
    STATE.with(|cell| {
        let mut x = cell.get();
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        cell.set(x);
        x
    })
}

/// Picks a duration uniformly at random from `[0, base]`.
///
/// `base` of zero returns zero — there is nothing to jitter.
///
/// `pub`, not `pub(crate)`: the live-price feed backs off its own
/// WebSocket reconnect attempts with jitter too, and duplicating this exact
/// generator in a second crate would be the same "no RNG crate for a narrow
/// need" reasoning fighting itself twice. Sharing this function does not
/// share the *rate budget* itself — a caller outside this crate still has no
/// way to touch [`crate::LimitGroup`]'s private token buckets or circuit
/// breaker, only this one stateless helper.
#[must_use]
pub fn full_jitter(base: Duration) -> Duration {
    if base.is_zero() {
        return base;
    }
    let span_nanos = u64::try_from(base.as_nanos()).unwrap_or(u64::MAX);
    let sampled = next_u64() % span_nanos.max(1);
    Duration::from_nanos(sampled)
}

#[cfg(test)]
mod tests {
    use super::full_jitter;
    use std::time::Duration;

    #[test]
    fn a_jittered_wait_never_exceeds_its_base() {
        let base = Duration::from_millis(250);
        for _ in 0..1000 {
            assert!(full_jitter(base) <= base);
        }
    }

    #[test]
    fn zero_jitters_to_zero() {
        assert_eq!(full_jitter(Duration::ZERO), Duration::ZERO);
    }

    #[test]
    fn repeated_jitter_from_the_same_base_is_not_constant() {
        // Full jitter samples the whole range, so drawing many values from
        // the same base and getting the same answer every time would mean
        // the generator is not actually random.
        let base = Duration::from_secs(1);
        let samples: std::collections::HashSet<Duration> =
            (0..50).map(|_| full_jitter(base)).collect();
        assert!(
            samples.len() > 1,
            "50 draws from the same base produced only one distinct value"
        );
    }
}
