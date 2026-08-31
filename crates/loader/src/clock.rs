//! [`SystemClock`] — the real-time [`Clock`] implementation.
//!
//! `senken-series::Clock` is deliberately introduced without one (see that
//! crate's module docs: a concrete implementation "belongs in whichever
//! crate first needs to run against real time — the loader, or the
//! runtime"). That is here: `senken-loader` is the first crate in this plan
//! that actually schedules work against wall-clock time (job `started_at`
//! stamps, throughput/ETA measurement, retry backoff), so it is the one
//! that must not read `SystemTime::now()`/`Instant::now()` directly outside
//! of this one file — every other module takes a `Clock` and calls
//! `clock.now()`/`clock.sleep_until()` instead, which is what keeps a
//! future backtest or replay run (a different `Clock` impl) deterministic.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use senken_core::UnixNanos;
use senken_series::Clock;

/// The wall clock, via `std::time::SystemTime`.
///
/// Live/interactive use only. A backtest or replay run supplies its own
/// [`Clock`] instead: that work is out of scope for now, and the seam exists
/// here specifically so it will not have to touch this crate.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

#[async_trait::async_trait]
impl Clock for SystemClock {
    fn now(&self) -> UnixNanos {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        // A `Duration` since the epoch always fits `i64` nanoseconds until
        // year 2262 (`UnixNanos`'s own documented range);
        // saturating rather than panicking here is strictly for a clock
        // that can never legitimately produce a value this large.
        UnixNanos::from_nanos(i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX))
    }

    async fn sleep_until(&self, t: UnixNanos) {
        let now = self.now();
        if t <= now {
            return;
        }
        // Both are `i64` nanoseconds and `t > now` was just checked, so the
        // difference is positive and representable.
        let remaining =
            Duration::from_nanos(u64::try_from(t.as_nanos() - now.as_nanos()).unwrap_or(0));
        tokio::time::sleep(remaining).await;
    }
}

#[cfg(test)]
/// A `Clock` a test fully controls: `now()` returns whatever was last set,
/// and `sleep_until` returns immediately rather than actually waiting —
/// exactly the property that makes backtest/replay `Clock`s deterministic
/// (design record Part IV.1), reused here so this crate's own tests never
/// need a real sleep to exercise retry backoff.
pub(crate) mod test_support {
    use std::sync::atomic::{AtomicI64, Ordering};

    use senken_core::UnixNanos;
    use senken_series::Clock;

    #[derive(Debug, Default)]
    pub(crate) struct ManualClock {
        now: AtomicI64,
    }

    impl ManualClock {
        pub(crate) fn at(nanos: i64) -> Self {
            Self {
                now: AtomicI64::new(nanos),
            }
        }
    }

    #[async_trait::async_trait]
    impl Clock for ManualClock {
        fn now(&self) -> UnixNanos {
            UnixNanos::from_nanos(self.now.load(Ordering::SeqCst))
        }

        async fn sleep_until(&self, t: UnixNanos) {
            // Deterministic tests must never actually wait; advancing the
            // clock to (at least) `t` is what a backtest/replay `Clock`
            // does too (`senken_series::Clock::sleep_until`'s own docs).
            let current = self.now.load(Ordering::SeqCst);
            if t.as_nanos() > current {
                self.now.store(t.as_nanos(), Ordering::SeqCst);
            }
        }
    }
}
