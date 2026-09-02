//! [`CompiledIndicatorInstance`]: one running instance of a component
//! compiled from indicator-lang source against `wit/senken.wit`'s
//! `compiled-indicator` world — the leaner sibling of
//! [`crate::instance::PluginInstance`] for a component that exports a bare
//! `on-bar` function instead of the `indicator` interface's descriptor and
//! `instance` resource.
//!
//! There is no guest-side `constructor` or `reset` to call here: every
//! incremental built-in a compiled program calls keeps its own state
//! host-side, keyed by call-site slot (`crate::builtins::BuiltinState`) and
//! living on this instance's own `Store` for as long as this type is —
//! so the state this instance accumulates across calls to [`Self::on_bar`]
//! *is* the entire indicator, and dropping this type (ordinary scope exit,
//! same as `PluginInstance`) discards it.

use std::sync::Arc;

use wasmtime::Store;

use crate::PluginHostError;
use crate::bindings::CompiledIndicator;
use crate::circuit::PluginCircuit;
use crate::health::RuntimeHealth;
use crate::instance::guarded_call;
use crate::log::{PluginLog, PluginLogLine};
use crate::wasi::PluginState;

/// One running instance of a component compiled from indicator-lang source.
pub struct CompiledIndicatorInstance {
    store: Store<PluginState>,
    plugin: CompiledIndicator,
    circuit: Arc<PluginCircuit>,
    log: PluginLog,
    health: Arc<RuntimeHealth>,
    /// See [`crate::instance::PluginInstance::last_fuel_consumed`] — the
    /// same fuel-accounting property, kept for the same reason.
    last_fuel_consumed: Option<u64>,
}

impl std::fmt::Debug for CompiledIndicatorInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledIndicatorInstance")
            .field("last_fuel_consumed", &self.last_fuel_consumed)
            .finish_non_exhaustive()
    }
}

impl CompiledIndicatorInstance {
    /// Wraps an already-instantiated component. There is no constructor
    /// call to make first — see this type's own doc comment for why.
    pub(crate) fn new(
        store: Store<PluginState>,
        plugin: CompiledIndicator,
        circuit: Arc<PluginCircuit>,
        log: PluginLog,
        health: Arc<RuntimeHealth>,
    ) -> Self {
        Self {
            store,
            plugin,
            circuit,
            log,
            health,
            last_fuel_consumed: None,
        }
    }

    /// Runs one bar through the compiled program's `plot` expression and
    /// returns its value. Bars must be handed to this in chronological
    /// order — the same requirement `wit/senken.wit` states for `on-bar`,
    /// since every built-in it calls back into is incremental.
    ///
    /// # Errors
    /// A [`PluginHostError::CircuitOpen`] if this plugin's breaker has since
    /// opened; a [`PluginHostError::Trap`] if this call itself traps,
    /// exceeds its epoch deadline, or exhausts its fuel budget. Either way
    /// this instance's `Store` is left intact and safe to drop.
    pub fn on_bar(
        &mut self,
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: f64,
    ) -> Result<f64, PluginHostError> {
        let Self {
            store,
            plugin,
            circuit,
            log,
            health,
            last_fuel_consumed,
        } = self;
        let (value, fuel_consumed) = guarded_call(circuit, log, health, store, |store| {
            plugin.call_on_bar(store, open, high, low, close, volume)
        })?;
        *last_fuel_consumed = fuel_consumed;
        Ok(value)
    }

    /// A snapshot of this instance's bounded log — see
    /// [`crate::instance::PluginInstance::logs`].
    #[must_use]
    pub fn logs(&self) -> Vec<PluginLogLine> {
        self.log.snapshot()
    }

    /// Fuel spent by the most recent [`Self::on_bar`]. `None` before the
    /// first call.
    #[must_use]
    pub fn last_fuel_consumed(&self) -> Option<u64> {
        self.last_fuel_consumed
    }
}
