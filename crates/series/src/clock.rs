//! [`Clock`] — where time comes from.
//!
//! Introduced here, not with the trade engine that will eventually consume
//! it: backtest, replay and live trading differ only in *where time comes
//! from*, and a wall-clock read anywhere in the bars/series stack would
//! make every consumer of it — including a future engine that has not been
//! designed yet — non-deterministic. This crate performs no I/O and reads
//! no wall clock anywhere for exactly this reason; every function that
//! needs "now" takes it as a parameter instead.
//!
//! This crate defines only the trait. A concrete implementation backed by
//! `std::time`/`tokio::time` belongs in whatever crate first needs to run
//! against real time (the loader, or the runtime) — adding one here would
//! mean either performing the I/O this crate promises not to, or adding a
//! runtime dependency this crate's `Cargo.toml` deliberately does not list.

use senken_core::UnixNanos;

/// An abstraction over "what time is it" and "wait until then".
///
/// `#[async_trait]` rather than a bare `async fn` in the trait: this must be
/// usable as `dyn Clock` (a backtest clock and a live clock are chosen at
/// runtime, not at compile time), and a bare `async fn` in a trait is not
/// dyn-compatible.
#[async_trait::async_trait]
pub trait Clock: Send + Sync {
    /// The current instant, as this clock understands it. For a live clock
    /// this reads the wall clock; for a backtest or replay clock it reads
    /// wherever that run's time actually comes from (the bar being
    /// processed, a fixed step, ...).
    fn now(&self) -> UnixNanos;

    /// Waits until `t` according to this clock. A live clock sleeps for the
    /// real difference; a backtest clock may return immediately having
    /// simply advanced its own notion of "now" to `t`.
    async fn sleep_until(&self, t: UnixNanos);
}
