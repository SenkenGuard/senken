//! Loads, runs, and **confines** compiled plugin components.
//!
//! The confinement is the point. A plugin is third-party code running inside
//! an application that will hold broker credentials, so the guarantee this
//! crate owes the rest of the system is narrow and absolute: **nothing a
//! plugin does may take the host down.**
//!
//! That guarantee is four separate mechanisms, and it fails if any one of
//! them is missing:
//!
//! - **No capabilities by default.** A component is granted nothing — no
//!   sockets, no HTTP, no filesystem. What is never handed over cannot be
//!   reached.
//! - **A memory ceiling.** The sandbox stops a guest escaping; it does not
//!   stop a guest exhausting the machine from inside it. That limit has to
//!   be installed deliberately, and nothing complains if it is forgotten.
//! - **Two execution limiters, for two different jobs.** Wall-clock epoch
//!   deadlines keep the application responsive while it is live; deterministic
//!   fuel keeps a backtest reproducible, because a backtest whose result
//!   moves between runs is a defect rather than noise.
//! - **A trap is an `Err`, not a death.** Which means no host code may
//!   `unwrap()` a guest result — that is where this guarantee is kept or
//!   lost.

mod bindings;
mod builtins;
mod circuit;
mod compiled_instance;
mod execution;
mod health;
mod host;
mod http_host;
mod instance;
mod log;
mod wasi;

pub use bindings::{
    Bar, BarSpec, BarUnit, BoxDrawable, Drawable, Extend, FetchError, IndicatorDescriptor,
    LabelAnchor, LabelDrawable, LevelDrawable, OnBarResult, ParamKind, ParamSpec, ParamValue,
    PlotPoint, PlotShape, PlotSpec, PlotValue, PriceCoord, Scaled, SegmentDrawable, SeriesDrawable,
    SeriesShape, VenueDescriptor, VenueError, VenueInstrument, Volume,
};
pub use circuit::CircuitState;
pub use compiled_instance::CompiledIndicatorInstance;
pub use execution::ExecutionMode;
pub use health::PluginHealth;
pub use host::{
    LoadedCompiledIndicator, LoadedPlugin, LoadedVenuePlugin, PluginHost, PluginLimits,
    SUPPORTED_API_VERSION, VenueCallError,
};
pub use instance::PluginInstance;
pub use log::{PluginLogLine, PluginLogSeverity};
// A caller building a `senken_venue::VenueClient` to hand to
// `PluginHost::load_venue` needs `senken-venue` itself (for
// `senken_venue::LimitGroup`) regardless of anything this crate could
// re-export, so `VenueClient` is named in `load_venue`'s own signature
// without a convenience re-export here.

/// What went wrong while loading or running a plugin.
///
/// Every variant is something the caller is expected to handle and report,
/// never something to unwrap: a plugin failing is ordinary, and the host
/// staying up through it is the entire contract.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginHostError {
    /// The component could not be loaded at all — malformed bytes, a world
    /// mismatch, or a capability it declares that this host does not grant.
    #[error("plugin could not be loaded: {0}")]
    Load(String),
    /// The guest trapped while running. The host is unaffected.
    #[error("plugin trapped while running: {0}")]
    Trap(String),
    /// This plugin's circuit breaker is open after repeated traps; the
    /// plugin stays disabled until a user explicitly re-enables it (see
    /// `crate::circuit`'s own docs for why this never clears on a timer).
    /// Carries the same human-readable reason the breaker recorded when it
    /// tripped.
    #[error("plugin disabled by its circuit breaker: {0}")]
    CircuitOpen(String),
    /// The compiled component names a `senken:plugin-api` version this host
    /// does not support. Distinguished from [`Self::Load`] because the two
    /// demand different fixes from a plugin author: recompiling against the
    /// version this host actually links, versus fixing bytes that are
    /// broken regardless of version.
    #[error(
        "plugin was compiled against plugin-api {found}, which this host does not support \
         (this host supports {supported})"
    )]
    Incompatible {
        /// The `senken:plugin-api@<version>` this component's imports or
        /// exports name.
        found: String,
        /// [`SUPPORTED_API_VERSION`] — the version this host's `Linker` and
        /// generated bindings were built against.
        supported: String,
    },
}
