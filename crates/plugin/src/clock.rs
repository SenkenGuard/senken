//! [`SystemClock`] — the real-time [`Clock`] a venue adapter closes candles
//! against.
//!
//! # Why it lives here
//!
//! [`senken_series::Clock`] is a trait with no concrete implementation on
//! purpose: `senken-series` performs no I/O and reads no wall clock, so
//! that every consumer of the bars stack — a backtest, a replay, a future
//! trade engine — stays deterministic. Its own docs say an implementation
//! "belongs in whatever crate first needs to run against real time".
//!
//! That crate is this one, and the reason is [`BarSource`](crate::BarSource)
//! itself. Most venues ship no flag saying whether the last candle in a
//! response has closed — no `confirm`, no `closeTime`, no server timestamp —
//! so the only way to avoid persisting a half-formed bar is to compare its
//! close time against now. Every adapter facing such a venue needs the same
//! few lines, and sixteen of them had written their own copy before this
//! existed: the same wall-clock read, the same saturating conversion, each
//! free to drift from the others.
//!
//! It is *not* in `senken-loader`, which also has one: that would drag
//! `senken-store` and its Arrow dependency graph into every venue plugin
//! for the sake of a `SystemTime::now()`.
//!
//! [`Clock`]: senken_series::Clock

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use senken_core::UnixNanos;
use senken_series::Clock;

/// The wall clock, via [`std::time::SystemTime`].
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

#[async_trait::async_trait]
impl Clock for SystemClock {
    fn now(&self) -> UnixNanos {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        // A `Duration` since the epoch fits `i64` nanoseconds until the year
        // 2262 — `UnixNanos`'s own documented range. Saturating rather than
        // panicking is for a clock that cannot legitimately reach that.
        UnixNanos::from_nanos(i64::try_from(elapsed.as_nanos()).unwrap_or(i64::MAX))
    }

    async fn sleep_until(&self, t: UnixNanos) {
        let now = self.now();
        if t <= now {
            return;
        }
        let remaining =
            Duration::from_nanos(u64::try_from(t.as_nanos() - now.as_nanos()).unwrap_or(0));
        tokio::time::sleep(remaining).await;
    }
}

#[cfg(test)]
mod tests {
    use super::SystemClock;
    use senken_series::Clock;

    #[test]
    fn the_clock_reads_a_time_after_this_projects_own_lifetime_began() {
        // Not a tautology: a clock that returned `UnixNanos::EPOCH` — which
        // a failed `duration_since` would — reads as 1970, and a bar source
        // comparing against it would treat every candle as long closed and
        // happily persist the still-forming one.
        let year_2020 = 1_577_836_800_000_000_000;
        assert!(SystemClock.now().as_nanos() > year_2020);
    }

    #[tokio::test]
    async fn sleeping_until_a_time_already_past_returns_at_once() {
        // A bar source that computed a negative wait must not block on it.
        let clock = SystemClock;
        let already_gone = clock.now();
        clock.sleep_until(already_gone).await;
    }
}
