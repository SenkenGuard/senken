//! Host-side bindings generated from `wit/senken.wit`.
//!
//! Generated the same way `crates/plugin-api`'s guest bindings are (see that
//! crate's `tests/host_bindgen.rs`, which proves the same file is
//! host-compilable): from the one WIT source of truth, never hand-edited.
//! `#[doc(hidden)]` for the same reason `senken_plugin_api::generated` is —
//! this is a path the rest of this crate needs, not a surface anyone should
//! write against, and it satisfies `missing_docs` honestly rather than
//! suppressing it, since `wasmtime::component::bindgen!` emits no doc
//! comments of its own.
#[doc(hidden)]
pub(crate) mod generated {
    wasmtime::component::bindgen!({
        path: "../../wit/senken.wit",
        world: "indicator-plugin",
    });
}

pub(crate) use generated::IndicatorPlugin;
pub use generated::exports::senken::plugin_api::indicator::{
    IndicatorDescriptor, OnBarResult, PlotValue,
};
pub use generated::senken::plugin_api::types::{
    Bar, BarSpec, BarUnit, BoxDrawable, Drawable, Extend, LabelAnchor, LabelDrawable,
    LevelDrawable, ParamKind, ParamSpec, ParamValue, PlotPoint, PlotShape, PlotSpec, PriceCoord,
    Scaled, SegmentDrawable, SeriesDrawable, SeriesShape, Volume,
};

/// Host-side bindings for `wit/senken.wit`'s `compiled-indicator` world —
/// the leaner target `senken_indicator_lang::compile` produces, which
/// exports a bare `on-bar` function and nothing that could describe itself
/// (no `descriptor`, no `instance` resource). Generated separately from
/// [`generated`] above because a `bindgen!` invocation is per-world; the two
/// modules share nothing at the Rust type level even though both import the
/// very same `builtins` interface — see `crate::host` for why that does not
/// require a second `add_to_linker` call.
#[doc(hidden)]
pub(crate) mod generated_compiled {
    wasmtime::component::bindgen!({
        path: "../../wit/senken.wit",
        world: "compiled-indicator",
    });
}

pub(crate) use generated_compiled::CompiledIndicator;
