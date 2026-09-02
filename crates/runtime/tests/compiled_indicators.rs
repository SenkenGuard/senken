//! Proves the end-to-end pipeline this task exists for: indicator-lang
//! source, compiled by `senken_indicator_lang::compile`, registered by
//! `senken_runtime::DynamicIndicators` through the `compiled-indicator`
//! bridge in `crates/runtime/src/plugin_host.rs`, and computed — producing
//! values identical to the equivalent built-in, bar for bar, the same
//! property `crates/indicator-lang/tests/equivalence.rs` and this crate's
//! own `tests/dynamic_indicators.rs` each prove for their own half of this
//! pipeline in isolation.
//!
//! Before this bridge existed, `DynamicIndicators::register` always failed
//! a `compiled-indicator` component with `PluginHostError::Load` — see
//! `crates/api/src/indicator_handlers.rs`'s
//! `a_valid_program_gets_past_the_compiler_and_fails_only_at_registration`,
//! which pinned that exact failure and is flipped to expect success
//! alongside this file.

use senken_core::UnixNanos;
use senken_indicators::{Ema, Indicator, MovingAverage};
use senken_runtime::{DynamicIndicatorError, DynamicIndicators};
use senken_series::{Bar, BarSpec, BarUnit, Volume};

fn bar(ts: i64, open: i64, high: i64, low: i64, close: i64, volume: i64) -> Bar {
    Bar {
        ts_open: UnixNanos::from_nanos(ts),
        open,
        high,
        low,
        close,
        volume: Volume::Real(volume),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// The same varied-every-field bar sequence
/// `crates/runtime/tests/dynamic_indicators.rs` uses for its own equivalence
/// proof of a Rust-authored dynamic indicator, reused here so a compiled one
/// is held to the identical bar set.
fn bars() -> Vec<Bar> {
    vec![
        bar(0, 100, 105, 95, 100, 10),
        bar(1, 101, 110, 98, 108, 20),
        bar(2, 108, 112, 100, 95, 5),
        bar(3, 95, 100, 90, 98, 15),
        bar(4, 98, 120, 96, 115, 30),
        bar(5, 115, 118, 108, 110, 8),
        bar(6, 110, 116, 104, 112, 12),
        bar(7, 112, 130, 111, 128, 40),
    ]
}

fn one_minute() -> BarSpec {
    BarSpec::new(1, BarUnit::Minute)
}

/// The whole task's reason for existing: compile, register and compute must
/// now succeed end to end, and the values a compiled `ema(close, 3)`
/// produces must match `senken_indicators::Ema` exactly.
#[test]
fn a_compiled_indicator_lang_program_matches_the_native_builtin_bar_for_bar() {
    let wasm = senken_indicator_lang::compile("plot ema(close, 3)\n").expect("source must compile");
    let catalog = DynamicIndicators::new().unwrap();
    let info = catalog.register(&wasm).expect(
        "a component compiled from valid indicator-lang source must register, not fail as it \
         did before this bridge existed",
    );
    assert!(
        info.params.is_empty(),
        "a compiled program has no runtime-configurable parameters"
    );
    assert_eq!(
        info.plots.len(),
        1,
        "the language has exactly one `plot` expression per program"
    );
    let field = info.plots[0].field.clone();

    let mut instance = catalog.spawn(&info.id, "{}").unwrap();
    let mut ema = Ema::new(3);

    for bar in bars() {
        ema.handle_bar(&bar);
        let on_bar = instance.handle_bar(&bar, one_minute()).unwrap();
        let value = on_bar
            .plots
            .iter()
            .find(|(name, _)| *name == field)
            .map(|(_, value)| *value)
            .expect("the compiled indicator always reports its one plot field");
        assert!(
            (value - ema.value()).abs() < 1e-9,
            "compiled ema {value} must match native Ema {} bar-for-bar",
            ema.value()
        );
    }
}

/// `senken_indicator_lang::compile` guarantees the same source always
/// produces byte-identical output, and this bridge's own id is a content
/// hash of that output (see `synthesize_compiled_info`) — so recompiling
/// and re-registering identical source must upsert the same catalog entry,
/// the same "re-uploading replaces the earlier registration" contract
/// `DynamicIndicators::register` already documents for an uploaded file.
#[test]
fn recompiling_the_same_source_registers_under_the_same_id() {
    let source = "plot rsi(3)\n";
    let catalog = DynamicIndicators::new().unwrap();

    let first = catalog
        .register(&senken_indicator_lang::compile(source).unwrap())
        .unwrap();
    let second = catalog
        .register(&senken_indicator_lang::compile(source).unwrap())
        .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(
        catalog.all().len(),
        1,
        "re-registering identical source must not leave two entries behind"
    );
}

/// Two different programs must never collide on the id
/// `synthesize_compiled_info` derives — proving the content hash actually
/// discriminates rather than always landing on some fixed value.
#[test]
fn two_different_programs_register_under_different_ids() {
    let catalog = DynamicIndicators::new().unwrap();
    let ema = catalog
        .register(&senken_indicator_lang::compile("plot ema(close, 3)\n").unwrap())
        .unwrap();
    let sma = catalog
        .register(&senken_indicator_lang::compile("plot sma(close, 3)\n").unwrap())
        .unwrap();
    assert_ne!(ema.id, sma.id);
    assert_eq!(catalog.all().len(), 2);
}

/// Bytes satisfying neither `wit/senken.wit` world are refused with a
/// message describing both rejections, not a panic — the same property
/// `crates/plugin-host/tests/sandbox.rs::malformed_bytes_fail_to_load_
/// without_taking_down_the_caller` proves one level down, checked again
/// here at the layer that now tries two worlds instead of one.
#[test]
fn bytes_satisfying_neither_world_are_rejected_with_a_message() {
    let catalog = DynamicIndicators::new().unwrap();
    let err = catalog.register(b"not a real component").unwrap_err();
    assert!(matches!(err, DynamicIndicatorError::Host(_)), "got {err:?}");
    let message = err.to_string();
    assert!(
        message.contains("indicator-plugin") && message.contains("compiled-indicator"),
        "the rejection must name both worlds it tried: {message:?}"
    );
}

/// A compiled indicator-lang program has no way to report warm-up, so
/// `initialized()` always reports `true` for one — see
/// `DynamicIndicatorInstance::initialized`'s own doc comment for why that
/// is the honest answer rather than an invented approximation. Checked here
/// so a future change does not quietly start gating a compiled indicator's
/// very first bars without anyone noticing.
#[test]
fn a_compiled_indicators_instance_is_always_reported_initialized() {
    let wasm = senken_indicator_lang::compile("plot ema(close, 20)\n").unwrap();
    let catalog = DynamicIndicators::new().unwrap();
    let info = catalog.register(&wasm).unwrap();
    let mut instance = catalog.spawn(&info.id, "{}").unwrap();

    // `ema(close, 20)` needs far more than one bar to converge, yet the
    // very first call must already report `true`.
    instance.handle_bar(&bars()[0], one_minute()).unwrap();
    assert!(instance.initialized().unwrap());
}
