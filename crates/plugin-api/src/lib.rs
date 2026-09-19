//! Public SDK for Senken plugins.
//!
//! A plugin author depends on this crate alone and gets every type needed
//! to implement an indicator against the `indicator-plugin` world defined
//! in `wit/senken.wit`: bars, scaled prices and quantities, plot points,
//! drawables, and the descriptor/instance contract a host calls into.
//! Implement [`Guest`] and [`GuestInstance`] on your own types, then call
//! [`export!`] once with the type that implements [`Guest`]:
//!
//! ```ignore
//! use senken_plugin_api::{Bar, Guest, GuestInstance, IndicatorDescriptor, OnBarResult, ParamValue};
//!
//! struct MyIndicator;
//!
//! impl Guest for MyIndicator {
//!     type Instance = MyInstance;
//!
//!     fn descriptor() -> IndicatorDescriptor {
//!         todo!()
//!     }
//! }
//!
//! struct MyInstance;
//!
//! impl GuestInstance for MyInstance {
//!     fn new(params: Vec<ParamValue>) -> Self { todo!() }
//!     fn handle_bar(&self, bar: Bar) -> OnBarResult { todo!() }
//!     fn initialized(&self) -> bool { todo!() }
//!     fn reset(&self) {}
//! }
//!
//! senken_plugin_api::export!(MyIndicator);
//! ```
//!
//! This crate never depends on a Senken domain crate — see its own
//! `Cargo.toml` for why, and [`convert`] for how the host side bridges
//! this crate's wire types to Senken's internal ones without that
//! dependency existing in either direction.
//!
//! `wit/senken.wit` is the one source of truth for what crosses the
//! plugin boundary; both this crate's guest bindings and the host's own
//! bindings (generated the same way, in `crates/plugin-host`) come from
//! it, and neither side hand-edits what its generator produced.

// `pub`, but not public API. `export!`'s expansion references
// `$crate::generated` (via `default_bindings_module` below) so it resolves
// from a plugin author's own crate, not just this one, and that reference
// fails to compile unless the path it names is reachable from outside here.
// `#[doc(hidden)]` says the rest: this module is a path the macro needs, not
// a surface anyone should write against — which matters more once this crate
// is published and its rendered documentation is what people read it by.
//
// It also happens to satisfy `missing_docs`, and that is the honest fix
// rather than suppressing the lint: `wit_bindgen::generate!` emits no doc
// comments, the same way `packages/web/src/lib/api/generated.ts` carries
// none, and the cure for an undocumented generated item is to change the WIT
// and regenerate — never to annotate output we do not hand-write.
#[doc(hidden)]
pub mod generated {
    wit_bindgen::generate!({
        path: "../../wit/senken.wit",
        world: "indicator-plugin",
        // The macro's default `export!` is only `pub(crate)`, which would
        // make `pub use generated::export;` below an error — a plugin
        // author outside this crate needs to call it.
        pub_export_macro: true,
        // Without this, `export!`'s expansion assumes it was generated at
        // the crate root and emits bare `generated::...` paths — those
        // resolve inside this crate's own tests by accident (there is
        // nothing else named `generated` to shadow it) but fail to
        // compile from a plugin author's crate, which has no such module.
        // `$crate` makes the path resolve to this crate specifically,
        // regardless of where `export!` is actually invoked.
        default_bindings_module: "$crate::generated",
    });
}

/// Guest bindings for `wit/senken.wit`'s `venue-plugin` world — the
/// dynamic counterpart to a compiled-in `senken_marketdata::MarketDataSource`/
/// `senken_plugin::BarSource` pair (this crate depends on neither, see the
/// module-level docs above).
///
/// A separate `generate!` invocation, not a second world folded into
/// [`generated`] above: `indicator-plugin` and `venue-plugin` are two
/// different worlds, each with its own export. `types` is aliased back to
/// [`generated`]'s own copy via `with` rather than left to generate a
/// second, structurally-identical-but-distinct `Bar`/`BarSpec` Rust type —
/// this world's `venue` interface reuses those two exactly
/// (`use types.{instant, bar, bar-spec}` in `wit/senken.wit`), so a plugin
/// that (hypothetically) implemented both worlds, or code in this crate
/// bridging one to the other, can hand either side the very same value.
/// `crates/plugin-host`'s own host-side bindings alias the two worlds'
/// `types` together the identical way, for the identical reason.
// `venue`'s one import, `http::fetch`, returns `result<list<u8>, ...>` —
// the one WIT shape this crate's `indicator-plugin` world never needed.
// `wit_bindgen::generate!`'s own guest-side lifting code for a `list<u8>`
// import result reconstructs it with `Vec::from_raw_parts(ptr, len, len)`,
// which is the correct canonical-ABI-lifting pattern (the allocation really
// was sized to exactly `len`) but reads to clippy as passing the same
// expression for a `Vec`'s length and capacity, a lint aimed at
// hand-written code that forgot to reserve extra capacity. This is macro
// output neither side hand-writes or can edit — see this crate's own
// module docs on why `generated`/`generated_venue` are `#[doc(hidden)]`
// rather than surfaces anyone maintains by hand — so the fix is to name the
// false positive here once, not to rewrite generated code to dodge a
// linter. Scoped to this one module, the only place in the workspace a
// `list<u8>` import crosses this macro.
#[allow(clippy::same_length_and_capacity)]
#[doc(hidden)]
pub mod generated_venue {
    wit_bindgen::generate!({
        path: "../../wit/senken.wit",
        world: "venue-plugin",
        pub_export_macro: true,
        default_bindings_module: "$crate::generated_venue",
        with: {
            "senken:plugin-api/types@0.1.0": crate::generated::senken::plugin_api::types,
        },
    });
}

pub mod convert;

pub use generated::export;
pub use generated::exports::senken::plugin_api::indicator::{
    Guest, GuestInstance, IndicatorDescriptor, OnBarResult, PlotValue,
};
pub use generated::senken::plugin_api::builtins::{
    atr_update, bollinger_update, ema_update, macd_update, rsi_update, sma_update,
    stochastic_update, volume_update, vwap_update, wma_update,
};
pub use generated::senken::plugin_api::types::{
    Bar, BarSpec, BarUnit, BoxDrawable, Drawable, Extend, LabelAnchor, LabelDrawable,
    LevelDrawable, ParamKind, ParamSpec, ParamValue, PlotPoint, PlotShape, PlotSpec, PriceCoord,
    Scaled, SegmentDrawable, SeriesDrawable, SeriesShape, Volume,
};

/// The guest export macro for a [`VenueGuest`] implementation — the
/// `venue-plugin` counterpart to [`export!`](export).
pub use generated_venue::export as export_venue;
pub use generated_venue::exports::senken::plugin_api::venue::{
    Contract, Guest as VenueGuest, Instrument, InstrumentKind, InstrumentStatus, OptionRight,
    OptionTerms, Settlement, VenueDescriptor, VenueError,
};
/// The one function a `venue-plugin` guest ever calls to reach the network:
/// resolved by the host against the one origin this plugin was loaded
/// for. See `wit/senken.wit`'s own `http` interface docs for the guarantee
/// this makes.
pub use generated_venue::senken::plugin_api::http::{FetchError, fetch};

/// The version of this SDK a plugin was compiled against.
///
/// A compiled component also carries this structurally, via the WIT
/// package version in `wit/senken.wit` (`senken:plugin-api@x.y.z`) — a
/// host can read that straight out of the component's type without
/// calling into it. This constant is the same number, exposed to Rust
/// source for a plugin author's own manifest or logging, so the two never
/// need to be kept in sync by hand.
pub const API_VERSION: &str = env!("CARGO_PKG_VERSION");
