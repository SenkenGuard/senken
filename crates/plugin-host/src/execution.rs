//! The two ways a call into a plugin can be bounded: wall-clock epoch
//! deadlines while live, deterministic fuel while backtesting.
//!
//! Both exist on the same [`wasmtime::Engine`] at once (`Config` enables
//! `epoch_interruption` and `consume_fuel` together — see
//! [`configure_engine`]), and each [`ExecutionMode`] sets one of them
//! tightly as the call's real bound and the other loosely as a backstop
//! that is not expected to fire. A live call is bounded by
//! [`ExecutionMode::Live`]'s wall-clock deadline, with an effectively
//! unreachable fuel ceiling behind it; a backtest call is bounded by
//! [`ExecutionMode::Backtest`]'s fuel budget, with an effectively
//! unreachable epoch deadline behind it. Running both mechanisms on every
//! call rather than swapping `Engine`s per mode means one compiled
//! component and one `Linker` serve both, at the cost of one epoch check
//! and one fuel check per instrumented point regardless of which mode is
//! active — cheap relative to a WASI host call, and worth it for not
//! duplicating everything this crate loads per plugin.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use wasmtime::{Config, Engine, Store};

/// How often the background ticker advances the engine's epoch while any
/// [`EpochTicker`] is running. Our own scheduling granularity, not a
/// correctness bound: a live deadline is only as precise as this interval,
/// so it is kept short enough that a one-second deadline still lands within
/// a small fraction of a second, and long enough that ticking is not a
/// measurable fraction of a CPU core on its own.
const EPOCH_TICK_INTERVAL: Duration = Duration::from_millis(10);

/// A fuel ceiling so far past any real backtest budget that a live call
/// bounded by its epoch deadline will trip that deadline long before it
/// could ever reach this many units of fuel. Its only job is to exist as
/// the inactive backstop in [`ExecutionMode::Live`].
///
/// Not `u64::MAX`: `wasmtime::Store::set_epoch_deadline` adds its argument to
/// the engine's *current* epoch, and `EFFECTIVELY_UNBOUNDED_EPOCH_TICKS`
/// below is added the same way — halving the range leaves room for that
/// addition without overflowing while still being unreachable in practice.
const EFFECTIVELY_UNBOUNDED_FUEL: u64 = u64::MAX / 2;

/// An epoch-tick count so far past any real live deadline that a backtest
/// call bounded by its fuel budget will exhaust that budget long before the
/// ticker could advance the engine's epoch this many times. Its only job is
/// to exist as the inactive backstop in [`ExecutionMode::Backtest`].
///
/// Not `u64::MAX`: see [`EFFECTIVELY_UNBOUNDED_FUEL`]'s doc comment — this
/// value is added to the engine's current epoch by
/// `wasmtime::Store::set_epoch_deadline`, which overflows if the sum would
/// exceed `u64::MAX`.
const EFFECTIVELY_UNBOUNDED_EPOCH_TICKS: u64 = u64::MAX / 2;

/// Enables the two interruption mechanisms this crate needs on `config`.
/// Call once, before the [`Engine`] is built — neither can be turned on
/// for a `Store` created from an `Engine` that did not enable it.
pub(crate) fn configure_engine(config: &mut Config) {
    config.epoch_interruption(true);
    config.consume_fuel(true);
}

/// How one call into a plugin is bounded.
#[derive(Debug, Clone, Copy)]
pub enum ExecutionMode {
    /// Bounded by wall-clock time, so a runaway plugin cannot freeze a live
    /// application: an `EpochTicker` advances the shared engine's epoch on
    /// a fixed interval, and this deadline is how many of those ticks the
    /// call is allowed to run for before it traps.
    Live {
        /// How long, in real time, a call is allowed to run. Rounded up to
        /// whole ticks of the epoch ticker's own interval.
        deadline: Duration,
    },
    /// Bounded by a fixed unit of work, so the same bar sequence costs the
    /// same on every run — a backtest whose result depends on how fast the
    /// machine happened to be that day is a defect, not acceptable noise.
    Backtest {
        /// The fuel budget for one call. Exhausting it traps the call.
        fuel: u64,
    },
}

impl ExecutionMode {
    /// Applies this mode's bound to `store`, arming the inactive mechanism
    /// with its unreachable backstop value.
    ///
    /// # Errors
    /// Only if `store`'s `Engine` was not configured with
    /// [`configure_engine`], which does not happen when every `Store` this
    /// crate creates comes from a [`crate::host::PluginHost`]'s own engine.
    pub(crate) fn apply<T>(self, store: &mut Store<T>) -> wasmtime::Result<()> {
        match self {
            Self::Live { deadline } => {
                let ticks = deadline
                    .as_nanos()
                    .div_ceil(EPOCH_TICK_INTERVAL.as_nanos())
                    .max(1);
                // `div_ceil` on nanosecond counts can exceed `u64` only for
                // a deadline far longer than any live call would ever be
                // configured with; saturating here is the same policy the
                // rest of this workspace uses for defensive arithmetic on a
                // value this far from its real operating range.
                let ticks = u64::try_from(ticks).unwrap_or(u64::MAX);
                store.set_epoch_deadline(ticks);
                store.set_fuel(EFFECTIVELY_UNBOUNDED_FUEL)?;
            }
            Self::Backtest { fuel } => {
                store.set_epoch_deadline(EFFECTIVELY_UNBOUNDED_EPOCH_TICKS);
                store.set_fuel(fuel)?;
            }
        }
        Ok(())
    }
}

/// A background thread that advances a shared [`Engine`]'s epoch on a fixed
/// interval, for as long as it is alive.
///
/// One ticker is started per [`crate::host::PluginHost`] and shared by
/// every plugin loaded through it — epoch ticks are engine-wide, not
/// per-`Store`, so one thread serves every live call regardless of how many
/// plugins are running. Dropping it stops and joins the thread; no plugin
/// call can be bounded by wall-clock time after that, which is why
/// `PluginHost` keeps this alive for as long as it itself is.
pub(crate) struct EpochTicker {
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl EpochTicker {
    pub(crate) fn start(engine: Engine) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Relaxed) {
                std::thread::sleep(EPOCH_TICK_INTERVAL);
                engine.increment_epoch();
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            // The thread wakes at most one `EPOCH_TICK_INTERVAL` after
            // `stop` is set; joining here is a bounded wait, not a hang.
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EpochTicker, configure_engine};
    use wasmtime::{Config, Engine};

    #[test]
    fn the_ticker_actually_advances_the_engines_epoch() {
        let mut config = Config::new();
        configure_engine(&mut config);
        let engine = Engine::new(&config).unwrap();
        let ticker = EpochTicker::start(engine.clone());
        // The property under test is that epochs advance at all, not a
        // specific count — proven by racing a real wasmtime deadline
        // against it rather than reading a private counter.
        let mut store = wasmtime::Store::new(&engine, ());
        store.set_epoch_deadline(1);
        // A trivial module with an epoch check point: a loop that yields to
        // the epoch check on every backward branch is the simplest way to
        // observe a trap without a real guest binary.
        let module =
            wasmtime::Module::new(&engine, r#"(module (func (export "spin") (loop br 0)))"#)
                .unwrap();
        let instance = wasmtime::Instance::new(&mut store, &module, &[]).unwrap();
        let spin = instance
            .get_typed_func::<(), ()>(&mut store, "spin")
            .unwrap();
        let result = spin.call(&mut store, ());
        drop(ticker);
        assert!(
            result.is_err(),
            "an epoch deadline of 1 tick must trap a spinning loop once the ticker advances it"
        );
    }
}
