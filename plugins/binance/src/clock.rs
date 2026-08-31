//! [`SystemClock`] — the real-time [`Clock`] this plugin closes candles
//! against.
//!
//! Binance's klines response carries no confirmation flag and no
//! server-time field — unlike OKX's `confirm` or Bybit's
//! top-level `time` — so the only way to know whether the last row has
//! actually closed is to compare its close time against "now" from
//! somewhere. `senken-series::Clock` is deliberately shipped without a
//! concrete implementation (its own module docs: one "belongs in whichever
//! crate first needs to run against real time"); this plugin is that
//! crate for Binance specifically, so the implementation lives here rather
//! than pulling in `senken-loader` (which would drag its
//! `senken-store`/Arrow dependency graph into every consumer of this
//! plugin) just to reuse the identical few lines `senken_loader::SystemClock`
//! already has.
//!
//! [`Clock`]: senken_series::Clock

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use senken_core::UnixNanos;
use senken_series::Clock;

/// The wall clock, via [`std::time::SystemTime`].
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct SystemClock;

#[async_trait::async_trait]
impl Clock for SystemClock {
    fn now(&self) -> UnixNanos {
        let elapsed = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or(Duration::ZERO);
        // A `Duration` since the epoch always fits `i64` nanoseconds until
        // year 2262 (`UnixNanos`'s own documented range); saturating rather
        // than panicking here is strictly for a clock that can never
        // legitimately produce a value this large.
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
