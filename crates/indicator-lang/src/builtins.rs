//! The language's entire standard library: the ten built-in indicators,
//! described once as data so the parser, the type checker and the code
//! generator all read the same table instead of each hard-coding their own
//! notion of what `ema` takes and returns.
//!
//! Every entry names the exact `senken_indicators` API it reuses (in its
//! doc comment) so a change to that crate's public shape is easy to find
//! here.

/// The shape of one built-in argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ParamKind {
    /// An arbitrary numeric expression, fed to the underlying moving
    /// average via `MovingAverage::update_raw` once per bar.
    Series,
    /// A non-negative whole number of bars, fixed at compile time — an
    /// indicator's period is a construction-time argument in
    /// `senken_indicators` (`Sma::new(period: usize)` and its siblings),
    /// never a value that changes bar to bar.
    Period,
    /// A decimal constant, fixed at compile time (Bollinger's band width).
    Number,
}

/// A bar field a built-in's host implementation reads directly from the
/// current bar — never written by a trader, because the built-ins that
/// need one (everything but the moving averages) call
/// `senken_indicators::Indicator::handle_bar`, which always reads the same
/// fixed fields off `Bar` itself rather than accepting a value for them.
/// `crate::codegen::module` appends these after a call's explicit
/// arguments, reading straight from `on-bar`'s own `open`/`high`/`low`/
/// `close`/`volume` parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImplicitArg {
    High,
    Low,
    Close,
    Volume,
}

impl ImplicitArg {
    /// The index of this field among `on-bar`'s own five parameters,
    /// matching `crate::typeck::BarField::param_index`.
    pub(crate) fn param_index(self) -> u32 {
        match self {
            ImplicitArg::High => 1,
            ImplicitArg::Low => 2,
            ImplicitArg::Close => 3,
            ImplicitArg::Volume => 4,
        }
    }
}

/// What a built-in call evaluates to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResultShape {
    /// A single number, usable directly in an expression.
    Scalar,
    /// More than one number, named exactly like the accessors on the
    /// `senken_indicators` type each field reuses. A call with this shape
    /// cannot be used directly — it must be narrowed with `.field` first.
    Compound(&'static [&'static str]),
}

/// One built-in's full signature.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Builtin {
    /// The name a trader writes in source, e.g. `ema`.
    pub(crate) name: &'static str,
    /// The function `wit/senken.wit`'s `builtins` interface declares for
    /// this built-in, e.g. `ema-update` — distinct from `name` because the
    /// WIT and the language's own naming convention differ (kebab-case
    /// host functions vs. bare source-level identifiers), and conflating
    /// them once produced a module that imported a function no host
    /// actually exports.
    pub(crate) host_fn: &'static str,
    /// Arguments a trader writes explicitly, in call order.
    pub(crate) params: &'static [ParamKind],
    /// Bar fields the host call additionally needs, appended after
    /// `params` — see [`ImplicitArg`]. Empty for `sma`/`ema`/`wma`, whose
    /// series argument already supplies whatever price the caller wants.
    pub(crate) implicit: &'static [ImplicitArg],
    pub(crate) result: ResultShape,
}

/// The ten built-ins, in the order `senken-indicators`' own docs list them.
pub(crate) const BUILTINS: &[Builtin] = &[
    // `Sma`/`Ema`/`Wma` — trend, overlay. All three drive
    // `MovingAverage::update_raw`, which is why they alone accept an
    // arbitrary series expression rather than always reading `close`.
    Builtin {
        name: "sma",
        host_fn: "sma-update",
        params: &[ParamKind::Series, ParamKind::Period],
        implicit: &[],
        result: ResultShape::Scalar,
    },
    Builtin {
        name: "ema",
        host_fn: "ema-update",
        params: &[ParamKind::Series, ParamKind::Period],
        implicit: &[],
        result: ResultShape::Scalar,
    },
    Builtin {
        name: "wma",
        host_fn: "wma-update",
        params: &[ParamKind::Series, ParamKind::Period],
        implicit: &[],
        result: ResultShape::Scalar,
    },
    // `Rsi` — momentum. `Rsi::handle_bar` always extracts `close` itself,
    // so unlike the moving averages there is no series argument to accept.
    Builtin {
        name: "rsi",
        host_fn: "rsi-update",
        params: &[ParamKind::Period],
        implicit: &[ImplicitArg::Close],
        result: ResultShape::Scalar,
    },
    // `Macd` — momentum, three values.
    Builtin {
        name: "macd",
        host_fn: "macd-update",
        params: &[ParamKind::Period, ParamKind::Period, ParamKind::Period],
        implicit: &[ImplicitArg::Close],
        result: ResultShape::Compound(&["macd", "signal", "histogram"]),
    },
    // `Stochastic` — momentum, two values.
    Builtin {
        name: "stochastic",
        host_fn: "stochastic-update",
        params: &[ParamKind::Period, ParamKind::Period],
        implicit: &[ImplicitArg::High, ImplicitArg::Low, ImplicitArg::Close],
        result: ResultShape::Compound(&["k", "d"]),
    },
    // `Atr` — volatility. `Atr::handle_bar` reads `high`/`low`/`close`
    // itself.
    Builtin {
        name: "atr",
        host_fn: "atr-update",
        params: &[ParamKind::Period],
        implicit: &[ImplicitArg::High, ImplicitArg::Low, ImplicitArg::Close],
        result: ResultShape::Scalar,
    },
    // `BollingerBands` — volatility, three values.
    Builtin {
        name: "bollinger",
        host_fn: "bollinger-update",
        params: &[ParamKind::Period, ParamKind::Number],
        implicit: &[ImplicitArg::Close],
        result: ResultShape::Compound(&["upper", "middle", "lower"]),
    },
    // `Vwap` — volume. `Vwap::handle_bar` reads the whole bar itself; no
    // arguments at all.
    Builtin {
        name: "vwap",
        host_fn: "vwap-update",
        params: &[],
        implicit: &[
            ImplicitArg::High,
            ImplicitArg::Low,
            ImplicitArg::Close,
            ImplicitArg::Volume,
        ],
        result: ResultShape::Scalar,
    },
    // `Volume` — volume. Likewise reads the bar itself.
    Builtin {
        name: "volume",
        host_fn: "volume-update",
        params: &[],
        implicit: &[ImplicitArg::Volume],
        result: ResultShape::Scalar,
    },
];

/// Looks up a built-in by name.
pub(crate) fn lookup(name: &str) -> Option<&'static Builtin> {
    BUILTINS.iter().find(|b| b.name == name)
}

/// The bar fields every program may read without a `let`, in the order
/// `on-bar` receives them as parameters.
pub(crate) const BAR_FIELDS: &[&str] = &["open", "high", "low", "close", "volume"];
