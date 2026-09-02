//! Proves `PluginHost::load_compiled`/`LoadedCompiledIndicator` — the second
//! load path this crate offers for a component compiled from
//! indicator-lang source against `wit/senken.wit`'s `compiled-indicator`
//! world, as opposed to a Rust-authored `indicator-plugin` — against real
//! compiled components rather than a description of what either world's
//! component would do.
//!
//! `senken_indicator_lang::compile` produces a `compiled-indicator`
//! component entirely in-process, so unlike `tests/sandbox.rs` this file
//! needs no `wasm32-wasip2` subprocess build for its main-line component;
//! `tests/support::build_fixture` is still used for the one
//! `indicator-plugin` component this file needs, to prove the two worlds
//! are told apart rather than silently accepted by the wrong loader.

mod support;

use senken_core::UnixNanos;
use senken_indicators::{Ema, Indicator, MovingAverage};
use senken_plugin_host::{ExecutionMode, PluginHost, PluginHostError, PluginLimits};
use senken_series::Bar;

/// A representative bar sequence that varies every OHLCV field, the same
/// shape `crates/indicator-lang/tests/equivalence.rs` uses for its own
/// bar-for-bar proof — reused here so this crate's sandboxed load path is
/// checked against the exact same property, not a weaker one. Kept as
/// small whole numbers, like that same file's own fixture, so both
/// directions this test needs (`f64::from` for `on_bar`, `i64::from` for
/// `senken_series::Bar`) are lossless widenings rather than a cast that
/// could truncate.
fn bars() -> Vec<(i32, i32, i32, i32, i32)> {
    vec![
        (100, 105, 95, 100, 10),
        (101, 110, 98, 108, 20),
        (108, 112, 100, 95, 5),
        (95, 100, 90, 98, 15),
        (98, 120, 96, 115, 30),
    ]
}

fn series_bar(open: i32, high: i32, low: i32, close: i32, volume: i32) -> Bar {
    Bar {
        ts_open: UnixNanos::EPOCH,
        open: i64::from(open),
        high: i64::from(high),
        low: i64::from(low),
        close: i64::from(close),
        volume: senken_series::Volume::Real(i64::from(volume)),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// The property this whole slice exists for: an indicator-lang program,
/// compiled and loaded through this crate's real sandbox (capability-zero
/// linker, memory ceiling, epoch/fuel bounds, circuit breaker) rather than a
/// bare `wasmtime::Linker` as `crates/indicator-lang`'s own equivalence
/// tests use, still computes the exact same numbers as the built-in it
/// calls, bar for bar.
#[test]
fn a_compiled_indicator_matches_the_equivalent_builtin_through_the_sandboxed_host() {
    let wasm = senken_indicator_lang::compile("plot ema(close, 3)\n").expect("source must compile");

    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host
        .load_compiled(&wasm)
        .expect("a valid compiled-indicator component must load");
    let mut instance = loaded
        .spawn(ExecutionMode::Backtest { fuel: 50_000_000 })
        .expect("spawning a fresh instance must succeed");

    let mut ema = Ema::new(3);
    for (open, high, low, close, volume) in bars() {
        let compiled_value = instance
            .on_bar(
                f64::from(open),
                f64::from(high),
                f64::from(low),
                f64::from(close),
                f64::from(volume),
            )
            .expect("a well-behaved compiled program must not trap");
        ema.handle_bar(&series_bar(open, high, low, close, volume));
        assert!(
            (compiled_value - ema.value()).abs() < 1e-9,
            "compiled ema {compiled_value} must match the native Ema {} bar-for-bar",
            ema.value()
        );
    }
}

/// This is the exact failure the whole task exists to close: a component
/// compiled by `senken_indicator_lang::compile` implements only
/// `compiled-indicator`, so loading it through the `indicator-plugin` world
/// — which expects an exported `indicator` interface no such component has
/// — must fail cleanly with a message, never panic.
#[test]
fn a_compiled_indicator_component_fails_to_load_via_the_indicator_plugin_world() {
    let wasm = senken_indicator_lang::compile("plot close\n").expect("source must compile");
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let err = host
        .load(&wasm)
        .expect_err("a compiled-indicator component has no `indicator` export to satisfy `load`");
    assert!(matches!(err, PluginHostError::Load(_)), "got {err:?}");
}

/// The reverse mismatch: an ordinary Rust-authored `indicator-plugin`
/// component has no bare `on-bar` export, so `load_compiled` must refuse it
/// with a message too, not silently accept a component that satisfies a
/// different world, and not panic while checking.
#[test]
fn an_indicator_plugin_component_fails_to_load_via_the_compiled_indicator_world() {
    let wasm = std::fs::read(support::build_fixture("deterministic")).unwrap();
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let err = host
        .load_compiled(&wasm)
        .expect_err("an indicator-plugin component has no bare `on-bar` export");
    assert!(matches!(err, PluginHostError::Load(_)), "got {err:?}");
}

/// Same fuel-bound wiring `tests/sandbox.rs` already proves for
/// `indicator-plugin` — checked again here because `load_compiled`/
/// `LoadedCompiledIndicator::spawn` apply `ExecutionMode` independently, and
/// nothing stops a future edit from forgetting to call `mode.apply` on this
/// second path specifically.
#[test]
fn an_exhausted_fuel_budget_traps_a_compiled_indicator_call() {
    let wasm = senken_indicator_lang::compile("plot ema(close, 3)\n").expect("source must compile");
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host.load_compiled(&wasm).unwrap();
    let mut instance = loaded
        .spawn(ExecutionMode::Backtest { fuel: 1 })
        .expect("spawning does not itself burn the call's own fuel budget");

    let err = instance
        .on_bar(100.0, 105.0, 95.0, 100.0, 10.0)
        .expect_err("one unit of fuel cannot cover a real builtin call");
    assert!(matches!(err, PluginHostError::Trap(_)), "got {err:?}");
}

/// The circuit breaker is the same shared, per-plugin mechanism
/// `tests/sandbox.rs` proves for `indicator-plugin` — this is the same
/// property, on the `compiled-indicator` path, so a plugin that keeps
/// trapping is disabled here too rather than being called into forever.
#[test]
fn repeated_traps_open_the_circuit_breaker_for_a_compiled_indicator_too() {
    let wasm = senken_indicator_lang::compile("plot ema(close, 3)\n").expect("source must compile");
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let loaded = host.load_compiled(&wasm).unwrap();
    let mut instance = loaded.spawn(ExecutionMode::Backtest { fuel: 1 }).unwrap();

    for _ in 0..3 {
        let err = instance
            .on_bar(100.0, 105.0, 95.0, 100.0, 10.0)
            .unwrap_err();
        assert!(matches!(err, PluginHostError::Trap(_)), "got {err:?}");
    }

    let err = instance
        .on_bar(100.0, 105.0, 95.0, 100.0, 10.0)
        .unwrap_err();
    assert!(
        matches!(err, PluginHostError::CircuitOpen(_)),
        "got {err:?}"
    );

    let err = loaded
        .spawn(ExecutionMode::Backtest { fuel: 50_000_000 })
        .expect_err("a tripped breaker must refuse new instances of the same plugin");
    assert!(
        matches!(err, PluginHostError::CircuitOpen(_)),
        "got {err:?}"
    );
}

/// Malformed bytes must fail to load through `load_compiled` the same way
/// they already do through `load` — never panic.
#[test]
fn malformed_bytes_fail_to_load_compiled_without_panicking() {
    let host = PluginHost::new(PluginLimits::default()).unwrap();
    let err = host
        .load_compiled(b"not a real component")
        .expect_err("garbage bytes must fail to load, not panic");
    assert!(matches!(err, PluginHostError::Load(_)), "got {err:?}");
}
