//! [`PluginInstance`]: one running, incrementally-updated indicator
//! instance, and the one place every guest call in this crate goes through
//! — which is where the trap-is-`Err` and circuit-breaker guarantees are
//! actually kept.

use std::sync::Arc;

use wasmtime::Store;
use wasmtime::component::ResourceAny;

use crate::PluginHostError;
use crate::bindings::{Bar, IndicatorPlugin, OnBarResult, ParamValue};
use crate::circuit::PluginCircuit;
use crate::health::RuntimeHealth;
use crate::log::{PluginLog, PluginLogLine, PluginLogSeverity};
use crate::wasi::PluginState;

/// One indicator instance from a [`crate::host::LoadedPlugin`].
///
/// Owns its `Store` outright, which is what makes the confinement real:
/// there is no path to this type that does not go through
/// [`crate::host::LoadedPlugin::spawn`], and dropping it (ordinary scope
/// exit, no method call required) drops the `Store` and with it every page
/// of the guest's linear memory. Nothing in this type's own `Drop`
/// implementation needs to free memory itself — `wasmtime::Store`'s own
/// drop glue already does that; this type's `Drop` only tears down the
/// guest-side resource handle first, as the WIT contract asks for.
pub struct PluginInstance {
    store: Store<PluginState>,
    plugin: IndicatorPlugin,
    handle: ResourceAny,
    circuit: Arc<PluginCircuit>,
    log: PluginLog,
    health: Arc<RuntimeHealth>,
    /// Fuel spent by the most recent call through this instance, if fuel
    /// accounting produced a number (it always does — this `Engine` always
    /// enables `consume_fuel`, see [`crate::execution::configure_engine`]).
    /// Reading this after two separate instances handle the same bar under
    /// [`crate::execution::ExecutionMode::Backtest`] is how the "same input,
    /// same fuel" property is checked from outside this crate.
    last_fuel_consumed: Option<u64>,
}

impl std::fmt::Debug for PluginInstance {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PluginInstance")
            .field("last_fuel_consumed", &self.last_fuel_consumed)
            .finish_non_exhaustive()
    }
}

impl PluginInstance {
    /// Constructs a new instance by calling the guest's own `constructor`
    /// with `params`, already instantiated into `store`.
    pub(crate) fn new(
        mut store: Store<PluginState>,
        plugin: IndicatorPlugin,
        params: &[ParamValue],
        circuit: Arc<PluginCircuit>,
        log: PluginLog,
        health: Arc<RuntimeHealth>,
    ) -> Result<Self, PluginHostError> {
        let (handle, fuel_consumed) = guarded_call(&circuit, &log, &health, &mut store, |store| {
            plugin
                .senken_plugin_api_indicator()
                .instance()
                .call_constructor(store, params)
        })?;
        Ok(Self {
            store,
            plugin,
            handle,
            circuit,
            log,
            health,
            last_fuel_consumed: fuel_consumed,
        })
    }

    /// Feeds one bar into this instance. Bars must be handed to this in
    /// chronological order — the same requirement `wit/senken.wit` places
    /// on the guest's own `handle-bar`, since this is a direct,
    /// unbuffered call into it.
    ///
    /// # Errors
    /// A [`PluginHostError::CircuitOpen`] if this plugin's breaker has
    /// since opened (from a trap on this or another instance of the same
    /// plugin); a [`PluginHostError::Trap`] if this call itself panics,
    /// exceeds its epoch deadline, exhausts its fuel budget, or otherwise
    /// traps. Either way this instance's `Store` is left intact and safe to
    /// drop — a trap never damages the host.
    pub fn handle_bar(&mut self, bar: Bar) -> Result<OnBarResult, PluginHostError> {
        let Self {
            store,
            plugin,
            handle,
            circuit,
            log,
            health,
            last_fuel_consumed,
        } = self;
        let (result, fuel_consumed) = guarded_call(circuit, log, health, store, |store| {
            plugin
                .senken_plugin_api_indicator()
                .instance()
                .call_handle_bar(store, *handle, bar)
        })?;
        *last_fuel_consumed = fuel_consumed;
        Ok(result)
    }

    /// Whether this instance has seen enough bars for its output to be
    /// meaningful.
    ///
    /// # Errors
    /// See [`Self::handle_bar`].
    pub fn initialized(&mut self) -> Result<bool, PluginHostError> {
        let Self {
            store,
            plugin,
            handle,
            circuit,
            log,
            health,
            ..
        } = self;
        let (result, _) = guarded_call(circuit, log, health, store, |store| {
            plugin
                .senken_plugin_api_indicator()
                .instance()
                .call_initialized(store, *handle)
        })?;
        Ok(result)
    }

    /// Returns this instance to the state it was in immediately after
    /// construction.
    ///
    /// # Errors
    /// See [`Self::handle_bar`].
    pub fn reset(&mut self) -> Result<(), PluginHostError> {
        let Self {
            store,
            plugin,
            handle,
            circuit,
            log,
            health,
            ..
        } = self;
        let ((), _) = guarded_call(circuit, log, health, store, |store| {
            plugin
                .senken_plugin_api_indicator()
                .instance()
                .call_reset(store, *handle)
        })?;
        Ok(())
    }

    /// A snapshot of this instance's bounded log — its own `stdout`/
    /// `stderr` output, interleaved with one line the host itself records
    /// for every trap, regardless of whether the guest printed anything.
    #[must_use]
    pub fn logs(&self) -> Vec<PluginLogLine> {
        self.log.snapshot()
    }

    /// Fuel spent by the most recent [`Self::handle_bar`] or (before the
    /// first bar) the constructor call. `None` only before either has run.
    #[must_use]
    pub fn last_fuel_consumed(&self) -> Option<u64> {
        self.last_fuel_consumed
    }
}

impl Drop for PluginInstance {
    fn drop(&mut self) {
        // Best-effort: the guest's own resource-drop glue could itself
        // trap, and that must not panic this `Drop` impl. Either way the
        // `Store` field (and with it every page of linear memory) is freed
        // immediately after this by Rust's own field drop order.
        let _ = self.handle.resource_drop(&mut self.store);
    }
}

/// Calls `call` with `store`, converting a trap into
/// [`PluginHostError::Trap`], recording it (and the fuel it cost) in `log`
/// and `health`, and feeding the outcome to `circuit` either way. This is
/// the one place in this crate that turns a raw `wasmtime::Result` from a
/// guest call into the `Result<_, PluginHostError>` every public method
/// above returns — every guest call in this crate goes through here, which
/// is what makes "no host code may `unwrap()` a guest result" a property of
/// the code rather than a convention someone has to remember at each call
/// site.
///
/// Generic over the `Store` data type `S` rather than fixed to
/// [`PluginState`]: [`crate::venue`]'s own venue-plugin calls go through
/// this exact same function, against its own, unrelated `VenuePluginState`
/// — the guarantee this function keeps ("a trap never damages the host")
/// does not depend on which world a `Store` was built for, only on never
/// letting a raw `wasmtime::Result` reach a caller unexamined.
pub(crate) fn guarded_call<S, T>(
    circuit: &PluginCircuit,
    log: &PluginLog,
    health: &RuntimeHealth,
    store: &mut Store<S>,
    call: impl FnOnce(&mut Store<S>) -> wasmtime::Result<T>,
) -> Result<(T, Option<u64>), PluginHostError> {
    circuit
        .ensure_closed()
        .map_err(PluginHostError::CircuitOpen)?;

    let fuel_before = store.get_fuel().ok();
    let outcome = call(store);
    let fuel_consumed = fuel_before
        .zip(store.get_fuel().ok())
        .map(|(before, after)| before.saturating_sub(after));

    match outcome {
        Ok(value) => {
            circuit.record_success();
            Ok((value, fuel_consumed))
        }
        Err(err) => {
            let message = format!("{err:#}");
            // `Trap::Interrupt` is specifically the epoch-deadline trap —
            // see `crate::execution::ExecutionMode::Live` — as opposed to a
            // guest panic, `Trap::OutOfFuel`, or a denied memory growth,
            // which is what makes this narrower than "the call failed" the
            // way `PluginHealth::deadline_exceeded_count` needs.
            let deadline_exceeded =
                err.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::Interrupt);
            health.record_trap(deadline_exceeded);
            // The per-plugin ring log (`log.record`) is what a person
            // looking at *this plugin* reads; this is the same event on the
            // operator-facing channel that spans every plugin the host is
            // running, for whoever is watching the application as a whole
            // rather than one plugin's own history.
            tracing::warn!(trap = %message, "plugin call trapped");
            log.record(PluginLogSeverity::Warn, format!("trap: {message}"));
            if let Some(reason) = circuit.record_trap(&message) {
                tracing::warn!(reason = %reason, "plugin circuit breaker opened");
                log.record(PluginLogSeverity::Warn, format!("circuit open: {reason}"));
            }
            Err(PluginHostError::Trap(message))
        }
    }
}
