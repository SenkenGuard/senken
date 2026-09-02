//! [`PluginHost`]: the entry point that turns compiled component bytes into
//! either a [`LoadedPlugin`] (the `indicator-plugin` world) or a
//! [`LoadedCompiledIndicator`] (the leaner `compiled-indicator` world
//! `senken_indicator_lang::compile` targets), and those two types
//! themselves, which turn a loaded component into a running
//! [`crate::instance::PluginInstance`] or
//! [`crate::compiled_instance::CompiledIndicatorInstance`] respectively.

use std::sync::Arc;
use std::time::Duration;

use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, Store};

use crate::PluginHostError;
use crate::bindings::{CompiledIndicator, IndicatorDescriptor, IndicatorPlugin, ParamValue};
use crate::builtins;
use crate::circuit::PluginCircuit;
use crate::compiled_instance::CompiledIndicatorInstance;
use crate::execution::{EpochTicker, ExecutionMode, configure_engine};
use crate::health::{PluginHealth, RuntimeHealth};
use crate::instance::PluginInstance;
use crate::log::PluginLog;
use crate::wasi::{PluginState, add_sandboxed_wasi_to_linker};

/// The `senken:plugin-api` version this host's `Linker` and generated
/// bindings (`crate::bindings`) were built against — see `wit/senken.wit`'s
/// own `package` declaration, which is this crate's single source of truth
/// for it and must be kept in step with this constant by hand, the same way
/// `crate::bindings`' `bindgen!` invocations already point at that same
/// file by hand.
///
/// A component naming any other version cannot link against this host no
/// matter how well-formed it otherwise is — the component model encodes a
/// WIT package's version into every one of its interfaces' import/export
/// names. Checking for that mismatch explicitly (see
/// `mismatched_api_version`) is what lets [`PluginHost::load`] answer
/// "recompile against this version" instead of the far less useful generic
/// "failed to instantiate" a raw linker error would otherwise produce for
/// exactly this case.
pub const SUPPORTED_API_VERSION: &str = "0.1.0";

/// If `component` imports or exports any `senken:plugin-api` interface at a
/// version other than [`SUPPORTED_API_VERSION`], returns that version.
///
/// `None` covers two different, both entirely normal cases this function
/// cannot and need not tell apart: a component that correctly names this
/// host's own version, and one that references `senken:plugin-api` under
/// none of its interfaces at all (garbage bytes, or a component built for a
/// foreign world) — either way, [`PluginHost::load`]'s ordinary
/// instantiation path is the right next step, and it already reports the
/// latter case as a plain load failure.
fn mismatched_api_version(component: &Component, engine: &Engine) -> Option<String> {
    let ty = component.component_type();
    ty.imports(engine)
        .map(|(name, _)| name)
        .chain(ty.exports(engine).map(|(name, _)| name))
        .find_map(|name| {
            let rest = name.strip_prefix("senken:plugin-api/")?;
            let version = rest.rsplit_once('@')?.1;
            (version != SUPPORTED_API_VERSION).then(|| version.to_owned())
        })
}

/// How long [`PluginHost::load`] gives a plugin's `descriptor` call before
/// giving up on it.
///
/// `descriptor` is declared in `wit/senken.wit` as static metadata a host
/// reads once, before constructing any instance — a legitimate
/// implementation returns immediately. This budget only exists so that a
/// plugin which does not honour that contract fails to *load* rather than
/// hanging whatever called `load`, including application startup itself.
const LOAD_PROBE_DEADLINE: Duration = Duration::from_secs(2);

/// Ceilings applied to every plugin loaded through one [`PluginHost`].
#[derive(Debug, Clone, Copy)]
pub struct PluginLimits {
    /// The linear-memory ceiling installed on every `Store` this host
    /// creates, in bytes.
    pub max_memory_bytes: usize,
}

impl Default for PluginLimits {
    fn default() -> Self {
        Self {
            // No measured plugin workload backs this number — it is a
            // conservative starting ceiling for one incremental indicator
            // instance, the same way `senken_venue::LimitGroup`'s own
            // default concurrency ceiling is a policy choice rather than a
            // fact about any real plugin. Small enough that a runaway
            // allocation is caught within a few dozen page-growth attempts,
            // large enough that a legitimate indicator's rolling-window
            // state is nowhere near it.
            max_memory_bytes: 32 * 1024 * 1024,
        }
    }
}

struct PluginHostInner {
    engine: Engine,
    linker: Linker<PluginState>,
    limits: PluginLimits,
    /// Kept alive for exactly as long as this host is — see
    /// [`EpochTicker`]'s own docs for why a live call's wall-clock deadline
    /// stops meaning anything once this is dropped.
    _epoch_ticker: EpochTicker,
}

/// Loads and confines compiled plugin components against one
/// capability-zero WASI surface.
///
/// Cheap to clone: every clone shares the same [`Engine`], [`Linker`] and
/// background epoch ticker, the same way `senken_venue::LimitGroup` shares
/// one budget across every client built from it.
#[derive(Clone)]
pub struct PluginHost {
    inner: Arc<PluginHostInner>,
}

impl PluginHost {
    /// Builds a new host: a component-model [`Engine`] with both epoch
    /// interruption and fuel consumption enabled (see this crate's own
    /// `execution` module), a [`Linker`] wired to exactly the capability-
    /// zero WASI surface in this crate's own `wasi` module and nothing past
    /// it, and a background thread advancing that engine's epoch for the
    /// lifetime of this host.
    ///
    /// # Errors
    /// If the underlying `wasmtime::Engine` or `Linker` construction
    /// fails — configuration this crate controls entirely, so this only
    /// happens if `wasmtime` itself rejects a setting this crate sets.
    pub fn new(limits: PluginLimits) -> Result<Self, PluginHostError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        configure_engine(&mut config);
        let engine = Engine::new(&config).map_err(|err| PluginHostError::Load(err.to_string()))?;
        let mut linker = Linker::new(&engine);
        add_sandboxed_wasi_to_linker(&mut linker)
            .map_err(|err| PluginHostError::Load(err.to_string()))?;
        builtins::add_to_linker(&mut linker)
            .map_err(|err| PluginHostError::Load(err.to_string()))?;
        let epoch_ticker = EpochTicker::start(engine.clone());
        Ok(Self {
            inner: Arc::new(PluginHostInner {
                engine,
                linker,
                limits,
                _epoch_ticker: epoch_ticker,
            }),
        })
    }

    /// Loads a compiled `wasm32-wasip2` component.
    ///
    /// This is where the capability-zero contract is actually enforced: the
    /// component is instantiated against this host's `Linker` immediately,
    /// so a component whose compiled imports reach past `wasi:cli`,
    /// `wasi:clocks`, `wasi:random` and `wasi:io` — `wasi:filesystem` or
    /// `wasi:sockets`, most importantly, since neither is ever linked —
    /// fails right here, before any guest code has run at all. The
    /// `descriptor` call that follows is bounded by a fixed probe deadline
    /// for the same reason: a plugin that hangs must fail to load, not hang
    /// whatever called this.
    ///
    /// # Errors
    /// A [`PluginHostError::Incompatible`] if the component names a
    /// `senken:plugin-api` version other than [`SUPPORTED_API_VERSION`]; a
    /// [`PluginHostError::Load`] if the bytes are not a valid component, if
    /// instantiation cannot satisfy every import this host's `Linker` does
    /// not provide (including any use of a capability this crate never
    /// grants), or if the `descriptor` call traps or exceeds its probe
    /// deadline.
    pub fn load(&self, wasm: &[u8]) -> Result<LoadedPlugin, PluginHostError> {
        self.try_load(wasm).inspect_err(|err| {
            // The operator-facing channel: a load failure is an event
            // about the host's plugin population as a whole, independent
            // of any one plugin's own ring log (which does not even exist
            // yet for a plugin that never finished loading).
            tracing::warn!(error = %err, "plugin failed to load");
        })
    }

    fn try_load(&self, wasm: &[u8]) -> Result<LoadedPlugin, PluginHostError> {
        let component = Component::new(&self.inner.engine, wasm)
            .map_err(|err| PluginHostError::Load(format!("not a valid component: {err}")))?;
        if let Some(found) = mismatched_api_version(&component, &self.inner.engine) {
            return Err(PluginHostError::Incompatible {
                found,
                supported: SUPPORTED_API_VERSION.to_owned(),
            });
        }

        let log = PluginLog::new();
        let health = Arc::new(RuntimeHealth::new());
        let mut store = Store::new(
            &self.inner.engine,
            PluginState::new(
                &log,
                self.inner.limits.max_memory_bytes,
                Arc::clone(&health),
            ),
        );
        store.limiter(|state| &mut state.limits);
        ExecutionMode::Live {
            deadline: LOAD_PROBE_DEADLINE,
        }
        .apply(&mut store)
        .map_err(|err| PluginHostError::Load(err.to_string()))?;

        let plugin = IndicatorPlugin::instantiate(&mut store, &component, &self.inner.linker)
            .map_err(|err| PluginHostError::Load(format!("failed to instantiate: {err:#}")))?;
        let descriptor = plugin
            .senken_plugin_api_indicator()
            .call_descriptor(&mut store)
            .map_err(|err| PluginHostError::Load(format!("descriptor call failed: {err:#}")))?;

        Ok(LoadedPlugin {
            host: self.clone(),
            component,
            descriptor,
            circuit: Arc::new(PluginCircuit::new()),
            log,
            health,
        })
    }

    /// Loads a compiled `wasm32-wasip2` component against `wit/senken.wit`'s
    /// `compiled-indicator` world instead of `indicator-plugin` — the world
    /// `senken_indicator_lang::compile` targets, which exports a bare
    /// `on-bar` function and nothing that could describe itself.
    ///
    /// Enforces exactly the same capability-zero surface, memory ceiling and
    /// linker as [`Self::load`]: the same `Engine`, the same `Linker`
    /// (already wired with the `builtins` import both worlds share — see
    /// this crate's own `builtins` module for why a second registration is
    /// not needed), the same memory limiter. There is no `descriptor` to
    /// probe, so unlike `load`, nothing here calls into the guest at all —
    /// instantiation against this host's capability-zero linker is already
    /// the whole check, exactly as it is for `indicator-plugin` before its
    /// own `descriptor` probe runs.
    ///
    /// # Errors
    /// A [`PluginHostError::Incompatible`] if the component names a
    /// `senken:plugin-api` version other than [`SUPPORTED_API_VERSION`]; a
    /// [`PluginHostError::Load`] if the bytes are not a valid component or
    /// if instantiation cannot satisfy every import this host's `Linker`
    /// does not provide — including a component that does not implement
    /// `compiled-indicator` at all (for instance, one built for
    /// `indicator-plugin` instead).
    pub fn load_compiled(&self, wasm: &[u8]) -> Result<LoadedCompiledIndicator, PluginHostError> {
        self.try_load_compiled(wasm).inspect_err(|err| {
            tracing::warn!(error = %err, "compiled indicator failed to load");
        })
    }

    fn try_load_compiled(&self, wasm: &[u8]) -> Result<LoadedCompiledIndicator, PluginHostError> {
        let component = Component::new(&self.inner.engine, wasm)
            .map_err(|err| PluginHostError::Load(format!("not a valid component: {err}")))?;
        if let Some(found) = mismatched_api_version(&component, &self.inner.engine) {
            return Err(PluginHostError::Incompatible {
                found,
                supported: SUPPORTED_API_VERSION.to_owned(),
            });
        }

        let log = PluginLog::new();
        let health = Arc::new(RuntimeHealth::new());
        let mut store = Store::new(
            &self.inner.engine,
            PluginState::new(
                &log,
                self.inner.limits.max_memory_bytes,
                Arc::clone(&health),
            ),
        );
        store.limiter(|state| &mut state.limits);
        ExecutionMode::Live {
            deadline: LOAD_PROBE_DEADLINE,
        }
        .apply(&mut store)
        .map_err(|err| PluginHostError::Load(err.to_string()))?;

        CompiledIndicator::instantiate(&mut store, &component, &self.inner.linker)
            .map_err(|err| PluginHostError::Load(format!("failed to instantiate: {err:#}")))?;

        Ok(LoadedCompiledIndicator {
            host: self.clone(),
            component,
            circuit: Arc::new(PluginCircuit::new()),
            log,
            health,
        })
    }
}

/// A component that has already proven it links against this host's
/// capability-zero surface and returned a valid descriptor.
///
/// Its circuit breaker, ring log and runtime health are all shared across
/// every [`PluginInstance`] spawned from it — "per plugin", not per
/// instance, the same scope `senken_venue::LimitGroup`'s own budget is
/// shared at. That scope is what makes any of the three worth reading here:
/// a compute request spawns and drops one instance per call, so a value
/// that lived only on the instance itself would never accumulate into
/// anything a person could look at afterwards.
pub struct LoadedPlugin {
    host: PluginHost,
    component: Component,
    descriptor: IndicatorDescriptor,
    circuit: Arc<PluginCircuit>,
    log: PluginLog,
    health: Arc<RuntimeHealth>,
}

impl std::fmt::Debug for LoadedPlugin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedPlugin")
            .field("id", &self.descriptor.id)
            .finish_non_exhaustive()
    }
}

impl LoadedPlugin {
    /// This plugin's static metadata, read once at [`PluginHost::load`]
    /// time.
    #[must_use]
    pub fn descriptor(&self) -> &IndicatorDescriptor {
        &self.descriptor
    }

    /// A snapshot of this plugin's own ring log — its every instance's
    /// `stdout`/`stderr`, interleaved with one line the host records for
    /// every trap and every circuit-breaker trip, oldest first.
    #[must_use]
    pub fn logs(&self) -> Vec<crate::log::PluginLogLine> {
        self.log.snapshot()
    }

    /// This plugin's current runtime health: total traps, how many of those
    /// were the wall-clock deadline specifically, peak granted memory, and
    /// the circuit breaker's own state — a pure read with no side effect
    /// (see `crate::circuit`'s own docs: this breaker never closes itself).
    #[must_use]
    pub fn health(&self) -> PluginHealth {
        self.health.snapshot(self.circuit.status())
    }

    /// Explicitly closes this plugin's circuit breaker if it is open,
    /// clearing its trap streak — the only way it ever recovers. Meant to
    /// be called from a user's own "re-enable" action once they have read
    /// why it tripped, never from a timer or from this crate itself; see
    /// `crate::circuit`'s own module docs for why a guest trap gets no
    /// automatic cooldown the way a venue's rate limit does.
    pub fn reset_circuit_breaker(&self) {
        self.circuit.reset();
    }

    /// Spawns a new, independent instance from `params`, bounded by `mode`
    /// for every call made through it.
    ///
    /// Fails immediately, without touching the plugin at all, if this
    /// plugin's circuit breaker is currently open from repeated traps on an
    /// earlier instance.
    ///
    /// # Errors
    /// A [`PluginHostError::CircuitOpen`] if the breaker is open; a
    /// [`PluginHostError::Load`] if a fresh instantiation of the same,
    /// already-validated component somehow fails (it should not, since
    /// `load` already proved it links); a [`PluginHostError::Trap`] if the
    /// constructor call itself traps.
    pub fn spawn(
        &self,
        params: &[ParamValue],
        mode: ExecutionMode,
    ) -> Result<PluginInstance, PluginHostError> {
        self.circuit
            .ensure_closed()
            .map_err(PluginHostError::CircuitOpen)?;

        let mut store = Store::new(
            &self.host.inner.engine,
            PluginState::new(
                &self.log,
                self.host.inner.limits.max_memory_bytes,
                Arc::clone(&self.health),
            ),
        );
        store.limiter(|state| &mut state.limits);
        mode.apply(&mut store)
            .map_err(|err| PluginHostError::Trap(err.to_string()))?;

        let plugin =
            IndicatorPlugin::instantiate(&mut store, &self.component, &self.host.inner.linker)
                .map_err(|err| PluginHostError::Load(format!("failed to instantiate: {err:#}")))?;

        PluginInstance::new(
            store,
            plugin,
            params,
            Arc::clone(&self.circuit),
            self.log.clone(),
            Arc::clone(&self.health),
        )
    }
}

/// A component that has already proven it links against this host's
/// capability-zero surface as a `compiled-indicator`.
///
/// Its circuit breaker, ring log and runtime health are all shared across
/// every [`crate::compiled_instance::CompiledIndicatorInstance`] spawned
/// from it — the same "per plugin, not per instance" scope [`LoadedPlugin`]'s
/// own fields are shared at, for the same reason.
pub struct LoadedCompiledIndicator {
    host: PluginHost,
    component: Component,
    circuit: Arc<PluginCircuit>,
    log: PluginLog,
    health: Arc<RuntimeHealth>,
}

impl std::fmt::Debug for LoadedCompiledIndicator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedCompiledIndicator")
            .finish_non_exhaustive()
    }
}

impl LoadedCompiledIndicator {
    /// A snapshot of this plugin's own ring log — see [`LoadedPlugin::logs`].
    #[must_use]
    pub fn logs(&self) -> Vec<crate::log::PluginLogLine> {
        self.log.snapshot()
    }

    /// This plugin's current runtime health — see [`LoadedPlugin::health`].
    #[must_use]
    pub fn health(&self) -> PluginHealth {
        self.health.snapshot(self.circuit.status())
    }

    /// Explicitly closes this plugin's circuit breaker — see
    /// [`LoadedPlugin::reset_circuit_breaker`].
    pub fn reset_circuit_breaker(&self) {
        self.circuit.reset();
    }

    /// Spawns a new, independent instance, bounded by `mode` for every call
    /// made through it. There are no parameters to pass — a compiled
    /// indicator-lang program has no way to declare any; whatever a trader
    /// wrote (a period, a multiplier) is already baked into the compiled
    /// bytes.
    ///
    /// Fails immediately, without touching the component at all, if this
    /// plugin's circuit breaker is currently open from repeated traps on an
    /// earlier instance.
    ///
    /// # Errors
    /// A [`PluginHostError::CircuitOpen`] if the breaker is open; a
    /// [`PluginHostError::Load`] if a fresh instantiation of the same,
    /// already-validated component somehow fails (it should not, since
    /// `load_compiled` already proved it links).
    pub fn spawn(&self, mode: ExecutionMode) -> Result<CompiledIndicatorInstance, PluginHostError> {
        self.circuit
            .ensure_closed()
            .map_err(PluginHostError::CircuitOpen)?;

        let mut store = Store::new(
            &self.host.inner.engine,
            PluginState::new(
                &self.log,
                self.host.inner.limits.max_memory_bytes,
                Arc::clone(&self.health),
            ),
        );
        store.limiter(|state| &mut state.limits);
        mode.apply(&mut store)
            .map_err(|err| PluginHostError::Trap(err.to_string()))?;

        let plugin =
            CompiledIndicator::instantiate(&mut store, &self.component, &self.host.inner.linker)
                .map_err(|err| PluginHostError::Load(format!("failed to instantiate: {err:#}")))?;

        Ok(CompiledIndicatorInstance::new(
            store,
            plugin,
            Arc::clone(&self.circuit),
            self.log.clone(),
            Arc::clone(&self.health),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::{PluginHost, PluginHostError, PluginLimits, SUPPORTED_API_VERSION};

    /// A hand-written component whose only import names a `senken:plugin-api`
    /// interface at a version this host does not support — proves
    /// [`super::mismatched_api_version`] against a real compiled component
    /// rather than a description of what one would look like, without
    /// needing an actual second copy of `wit/senken.wit` at another version
    /// (this crate's WAT-text fixture has no idea what `types` actually
    /// exports; the import's *name* is the only thing under test, since
    /// that name alone is what the component model encodes a WIT package's
    /// version into).
    fn wat_component_naming_version(version: &str) -> Vec<u8> {
        format!(
            r#"(component
                 (import "senken:plugin-api/types@{version}" (instance))
               )"#
        )
        .into_bytes()
    }

    #[test]
    fn a_component_naming_a_different_plugin_api_version_is_reported_as_incompatible() {
        let host = PluginHost::new(PluginLimits::default()).unwrap();
        let wasm = wat_component_naming_version("9.9.9");
        let err = host
            .load(&wasm)
            .expect_err("a mismatched plugin-api version must not load");
        match err {
            PluginHostError::Incompatible { found, supported } => {
                assert_eq!(found, "9.9.9");
                assert_eq!(supported, SUPPORTED_API_VERSION);
            }
            other => panic!("expected Incompatible, got {other:?}"),
        }

        // The same check applies to the leaner `compiled-indicator` load
        // path, not only `indicator-plugin` — both worlds live in the same
        // versioned package.
        let err = host.load_compiled(&wasm).expect_err(
            "a mismatched plugin-api version must not load as a compiled indicator either",
        );
        assert!(
            matches!(err, PluginHostError::Incompatible { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn a_component_naming_this_hosts_own_version_is_not_reported_as_incompatible() {
        // It still fails to load — this component exports nothing
        // `indicator-plugin` needs — but that failure must be a plain
        // `Load`, not `Incompatible`: the version it names is exactly the
        // one this host supports.
        let host = PluginHost::new(PluginLimits::default()).unwrap();
        let wasm = wat_component_naming_version(SUPPORTED_API_VERSION);
        let err = host.load(&wasm).unwrap_err();
        assert!(matches!(err, PluginHostError::Load(_)), "got {err:?}");
    }

    #[test]
    fn a_component_naming_no_plugin_api_version_at_all_is_a_plain_load_failure() {
        // No `senken:plugin-api` reference anywhere — the ordinary case of
        // a component built for a totally unrelated world — must fall
        // through to the generic instantiation failure, not a fabricated
        // version claim.
        let host = PluginHost::new(PluginLimits::default()).unwrap();
        let wasm = b"(component)".to_vec();
        let err = host.load(&wasm).unwrap_err();
        assert!(matches!(err, PluginHostError::Load(_)), "got {err:?}");
    }
}
