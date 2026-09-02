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

/// Host-side bindings for `wit/senken.wit`'s `venue-plugin` world — the
/// dynamic counterpart to a compiled-in [`senken_plugin::MarketDataSource`]/
/// [`senken_plugin::BarSource`] pair.
///
/// `types` is aliased back to [`generated`]'s own copy via `with` rather
/// than left to generate a second, structurally-identical-but-distinct
/// `Bar`/`BarSpec` Rust type: this world's `venue` interface reuses those
/// two exactly (`use types.{instant, bar, bar-spec}` in `wit/senken.wit`),
/// and `senken_runtime::plugin_host`'s bridge functions
/// (`bar_to_wit`/`bar_spec_to_wit`) must be able to hand either world's
/// bindings the very same value.
#[doc(hidden)]
pub(crate) mod generated_venue {
    wasmtime::component::bindgen!({
        path: "../../wit/senken.wit",
        world: "venue-plugin",
        with: {
            "senken:plugin-api/types@0.1.0": crate::bindings::generated::senken::plugin_api::types,
        },
    });
}

pub(crate) use generated_venue::VenuePlugin;
pub use generated_venue::exports::senken::plugin_api::venue::{
    Instrument as VenueInstrument, VenueDescriptor, VenueError,
};
pub use generated_venue::senken::plugin_api::http::FetchError;
pub(crate) use generated_venue::senken::plugin_api::http::Host as HttpHost;
