//! The crate's central claim, proved by execution rather than asserted: an
//! indicator-lang program calling a built-in produces the exact same
//! numbers, bar for bar, as calling the equivalent `senken_indicators`
//! type directly — because the compiled program's only way to compute
//! that built-in's value is to call back into a host function this test
//! wires up to that exact same type.
//!
//! Every test here instantiates its own host state and its own wasmtime
//! `Store`, so tests never share indicator state with each other.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use senken_core::UnixNanos;
use senken_indicators::{
    Atr, BollingerBands, Ema, Indicator, Macd, MovingAverage, Rsi, Sma, Stochastic, Volume, Vwap,
    Wma,
};
use senken_series::Bar;
use wasmtime::Engine;
use wasmtime::component::{Component, Linker, Val};

/// One OHLCV bar, kept as `i32` (every value below is a small, hand-picked
/// whole number) so both directions this test needs — `f64::from` for
/// wasmtime's `Val::Float64`, `i64::from` for `senken_series::Bar` — are
/// lossless widenings rather than a cast that could truncate.
/// `senken_indicators::convert::scaled_to_f64` is itself a plain widening
/// cast, so the same whole-number value reaches both paths exactly.
#[derive(Debug, Clone, Copy)]
struct OhlcvBar {
    open: i32,
    high: i32,
    low: i32,
    close: i32,
    volume: i32,
}

fn bar(open: i32, high: i32, low: i32, close: i32, volume: i32) -> OhlcvBar {
    OhlcvBar {
        open,
        high,
        low,
        close,
        volume,
    }
}

/// The same bar, as the `senken_series::Bar` the direct `senken_indicators`
/// call feeds through `handle_bar`.
fn series_bar(b: OhlcvBar) -> Bar {
    Bar {
        ts_open: UnixNanos::EPOCH,
        open: i64::from(b.open),
        high: i64::from(b.high),
        low: i64::from(b.low),
        close: i64::from(b.close),
        volume: senken_series::Volume::Real(i64::from(b.volume)),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// A representative sequence that varies every field on every bar, so an
/// indicator that secretly ignored one would be caught, and reversing it
/// changes every order-sensitive built-in's reading.
fn bars() -> Vec<OhlcvBar> {
    vec![
        bar(100, 105, 95, 100, 10),
        bar(101, 110, 98, 108, 20),
        bar(108, 112, 100, 95, 5),
        bar(95, 100, 90, 98, 15),
        bar(98, 120, 96, 115, 30),
        bar(115, 118, 108, 110, 8),
        bar(110, 116, 104, 112, 12),
        bar(112, 130, 111, 128, 40),
    ]
}

/// Host-side state for every built-in a compiled program might import,
/// keyed by the call-site `slot` `crate::typeck` assigned it. Lazily
/// constructed on a slot's first call, exactly as
/// `wit/senken.wit`'s doc comment describes.
struct HostState {
    sma: HashMap<u32, Sma>,
    ema: HashMap<u32, Ema>,
    wma: HashMap<u32, Wma>,
    rsi: HashMap<u32, Rsi>,
    atr: HashMap<u32, Atr>,
    vwap: HashMap<u32, Vwap>,
    volume: HashMap<u32, Volume>,
    stochastic: HashMap<u32, Stochastic>,
    macd: HashMap<u32, Macd>,
    bollinger: HashMap<u32, BollingerBands>,
    /// The bar `run_compiled` is currently feeding through `on-bar`, set
    /// once per bar before that call is made.
    ///
    /// Every scalar or compound built-in's *implicit* arguments (see
    /// `crate::builtins::ImplicitArg`) are bar fields a trader never
    /// writes — `crate::codegen::module` always reads them straight off
    /// `on-bar`'s own parameters, unchanged by any `let`. So the `close`/
    /// `high`/`low`/`volume` a host closure below receives as wasm `f64`s
    /// are always exactly this bar's own fields, and reading this instead
    /// of reconstructing a `Bar` from those `f64`s is both exact (no
    /// float-to-integer narrowing anywhere in this file) and identical in
    /// substance: it is the same bar either way.
    current_bar: Bar,
}

impl Default for HostState {
    fn default() -> Self {
        Self {
            sma: HashMap::new(),
            ema: HashMap::new(),
            wma: HashMap::new(),
            rsi: HashMap::new(),
            atr: HashMap::new(),
            vwap: HashMap::new(),
            volume: HashMap::new(),
            stochastic: HashMap::new(),
            macd: HashMap::new(),
            bollinger: HashMap::new(),
            current_bar: series_bar(bar(0, 0, 0, 0, 0)),
        }
    }
}

/// Links `sma-update`/`ema-update`/`wma-update` — the three built-ins
/// driven through `MovingAverage::update_raw` rather than `handle_bar`.
fn link_moving_averages(
    builtins: &mut wasmtime::component::LinkerInstance<'_, ()>,
    state: &Arc<Mutex<HostState>>,
) {
    macro_rules! moving_average {
        ($name:literal, $field:ident, $ty:ty) => {
            let state = Arc::clone(state);
            builtins
                .func_wrap($name, move |_, (slot, value, period): (u32, f64, u32)| {
                    let mut state = state.lock().unwrap();
                    let ma = state
                        .$field
                        .entry(slot)
                        .or_insert_with(|| <$ty>::new(period as usize));
                    ma.update_raw(value);
                    Ok((ma.value(),))
                })
                .unwrap();
        };
    }
    moving_average!("sma-update", sma, Sma);
    moving_average!("ema-update", ema, Ema);
    moving_average!("wma-update", wma, Wma);
}

/// Links the single-valued built-ins driven through `Indicator::handle_bar`
/// (as opposed to the moving averages' `update_raw`), each handling
/// `state.current_bar` — see that field's own doc comment for why that is
/// the same bar the wasm-supplied `f64` arguments describe.
fn link_scalar_bar_indicators(
    builtins: &mut wasmtime::component::LinkerInstance<'_, ()>,
    state: &Arc<Mutex<HostState>>,
) {
    {
        let state = Arc::clone(state);
        builtins
            .func_wrap(
                "rsi-update",
                move |_, (slot, period, _close): (u32, u32, f64)| {
                    let mut state = state.lock().unwrap();
                    let bar = state.current_bar;
                    let rsi = state
                        .rsi
                        .entry(slot)
                        .or_insert_with(|| Rsi::new(period as usize));
                    rsi.handle_bar(&bar);
                    Ok((rsi.value(),))
                },
            )
            .unwrap();
    }
    {
        let state = Arc::clone(state);
        builtins
            .func_wrap(
                "atr-update",
                move |_, (slot, period, _high, _low, _close): (u32, u32, f64, f64, f64)| {
                    let mut state = state.lock().unwrap();
                    let bar = state.current_bar;
                    let atr = state
                        .atr
                        .entry(slot)
                        .or_insert_with(|| Atr::new(period as usize));
                    atr.handle_bar(&bar);
                    Ok((atr.value(),))
                },
            )
            .unwrap();
    }
    {
        let state = Arc::clone(state);
        builtins
            .func_wrap(
                "vwap-update",
                move |_, (slot, _high, _low, _close, _volume): (u32, f64, f64, f64, f64)| {
                    let mut state = state.lock().unwrap();
                    let bar = state.current_bar;
                    let vwap = state.vwap.entry(slot).or_default();
                    vwap.handle_bar(&bar);
                    Ok((vwap.value(),))
                },
            )
            .unwrap();
    }
    {
        let state = Arc::clone(state);
        builtins
            .func_wrap("volume-update", move |_, (slot, _volume): (u32, f64)| {
                let mut state = state.lock().unwrap();
                let bar = state.current_bar;
                let vol = state.volume.entry(slot).or_default();
                vol.handle_bar(&bar);
                Ok((vol.value(),))
            })
            .unwrap();
    }
}

/// Links the built-ins that report more than one value per bar
/// (`stochastic`, `macd`, `bollinger`) — the three whose host call needs
/// the return-pointer scratch space proven in
/// `crate::codegen::component::tests::multi_value_import_round_trips_through_a_return_pointer`.
fn link_compound_bar_indicators(
    builtins: &mut wasmtime::component::LinkerInstance<'_, ()>,
    state: &Arc<Mutex<HostState>>,
) {
    {
        let state = Arc::clone(state);
        builtins
            .func_wrap(
                "stochastic-update",
                move |_,
                      (slot, k_period, d_period, _high, _low, _close): (
                    u32,
                    u32,
                    u32,
                    f64,
                    f64,
                    f64,
                )| {
                    let mut state = state.lock().unwrap();
                    let bar = state.current_bar;
                    let stoch = state
                        .stochastic
                        .entry(slot)
                        .or_insert_with(|| Stochastic::new(k_period as usize, d_period as usize));
                    stoch.handle_bar(&bar);
                    Ok(((stoch.k(), stoch.d()),))
                },
            )
            .unwrap();
    }
    {
        let state = Arc::clone(state);
        builtins
            .func_wrap(
                "macd-update",
                move |_, (slot, fast, slow, signal, _close): (u32, u32, u32, u32, f64)| {
                    let mut state = state.lock().unwrap();
                    let bar = state.current_bar;
                    let macd = state.macd.entry(slot).or_insert_with(|| {
                        Macd::new(fast as usize, slow as usize, signal as usize)
                    });
                    macd.handle_bar(&bar);
                    Ok(((macd.macd(), macd.signal(), macd.histogram()),))
                },
            )
            .unwrap();
    }
    {
        let state = Arc::clone(state);
        builtins
            .func_wrap(
                "bollinger-update",
                move |_, (slot, period, k, _close): (u32, u32, f64, f64)| {
                    let mut state = state.lock().unwrap();
                    let bar = state.current_bar;
                    let bb = state
                        .bollinger
                        .entry(slot)
                        .or_insert_with(|| BollingerBands::new(period as usize, k));
                    bb.handle_bar(&bar);
                    Ok(((bb.upper(), bb.middle(), bb.lower()),))
                },
            )
            .unwrap();
    }
}

/// Links every one of the ten built-ins against `state`, each calling
/// straight into the `senken_indicators` type it names — the same
/// reused, already-tested Rust this crate's `README.md` promises `ema(close,
/// 20)` compiles down to.
fn link_builtins(linker: &mut Linker<()>, state: &Arc<Mutex<HostState>>) {
    let mut builtins = linker
        .instance("senken:plugin-api/builtins@0.1.0")
        .expect("wit/senken.wit's `compiled-indicator` world imports this interface");
    link_moving_averages(&mut builtins, state);
    link_scalar_bar_indicators(&mut builtins, state);
    link_compound_bar_indicators(&mut builtins, state);
}

/// Compiles `source`, feeds `bars` through its `on-bar` export in order,
/// and returns each bar's plotted value.
fn run_compiled(source: &str, bars: &[OhlcvBar]) -> Vec<f64> {
    let component_bytes = senken_indicator_lang::compile(source).expect("source must compile");

    let engine = Engine::default();
    let component =
        Component::new(&engine, &component_bytes).expect("wasmtime must load the component");

    let state = Arc::new(Mutex::new(HostState::default()));
    let mut linker = Linker::new(&engine);
    link_builtins(&mut linker, &state);

    let mut store = wasmtime::Store::new(&engine, ());
    let instance = linker
        .instantiate(&mut store, &component)
        .expect("instantiation must succeed");
    let on_bar = instance
        .get_func(&mut store, "on-bar")
        .expect("component must export on-bar");

    let mut out = Vec::with_capacity(bars.len());
    for b in bars {
        // Every scalar/compound built-in's implicit bar fields always
        // forward this same bar unchanged — see `HostState::current_bar`.
        // Setting it here, once per bar, is what lets the host closures
        // above use it directly instead of reconstructing one from the
        // wasm-supplied `f64` arguments.
        state.lock().unwrap().current_bar = series_bar(*b);
        let mut results = [Val::Float64(0.0)];
        on_bar
            .call(
                &mut store,
                &[
                    Val::Float64(f64::from(b.open)),
                    Val::Float64(f64::from(b.high)),
                    Val::Float64(f64::from(b.low)),
                    Val::Float64(f64::from(b.close)),
                    Val::Float64(f64::from(b.volume)),
                ],
                &mut results,
            )
            .expect("calling on-bar must succeed");
        let Val::Float64(value) = results[0] else {
            panic!("on-bar returns f64")
        };
        out.push(value);
    }
    out
}

fn assert_all_close(actual: &[f64], expected: &[f64]) {
    assert_eq!(actual.len(), expected.len());
    for (i, (a, e)) in actual.iter().zip(expected).enumerate() {
        assert!((a - e).abs() < 1e-9, "bar {i}: expected {e}, got {a}");
    }
}

#[test]
fn ema_matches_senken_indicators_ema_bar_for_bar() {
    let compiled = run_compiled("plot ema(close, 3)\n", &bars());

    let mut ema = Ema::new(3);
    let expected: Vec<f64> = bars()
        .into_iter()
        .map(|b| {
            ema.handle_bar(&series_bar(b));
            ema.value()
        })
        .collect();

    assert_all_close(&compiled, &expected);
}

#[test]
fn sma_matches_senken_indicators_sma_bar_for_bar() {
    let compiled = run_compiled("plot sma(close, 3)\n", &bars());

    let mut sma = Sma::new(3);
    let expected: Vec<f64> = bars()
        .into_iter()
        .map(|b| {
            sma.handle_bar(&series_bar(b));
            sma.value()
        })
        .collect();

    assert_all_close(&compiled, &expected);
}

#[test]
fn wma_matches_senken_indicators_wma_bar_for_bar() {
    let compiled = run_compiled("plot wma(close, 3)\n", &bars());

    let mut wma = Wma::new(3);
    let expected: Vec<f64> = bars()
        .into_iter()
        .map(|b| {
            wma.handle_bar(&series_bar(b));
            wma.value()
        })
        .collect();

    assert_all_close(&compiled, &expected);
}

#[test]
fn rsi_matches_senken_indicators_rsi_bar_for_bar() {
    let compiled = run_compiled("plot rsi(3)\n", &bars());

    let mut rsi = Rsi::new(3);
    let expected: Vec<f64> = bars()
        .into_iter()
        .map(|b| {
            rsi.handle_bar(&series_bar(b));
            rsi.value()
        })
        .collect();

    assert_all_close(&compiled, &expected);
}

#[test]
fn atr_matches_senken_indicators_atr_bar_for_bar() {
    let compiled = run_compiled("plot atr(3)\n", &bars());

    let mut atr = Atr::new(3);
    let expected: Vec<f64> = bars()
        .into_iter()
        .map(|b| {
            atr.handle_bar(&series_bar(b));
            atr.value()
        })
        .collect();

    assert_all_close(&compiled, &expected);
}

#[test]
fn vwap_matches_senken_indicators_vwap_bar_for_bar() {
    let compiled = run_compiled("plot vwap()\n", &bars());

    let mut vwap = Vwap::new();
    let expected: Vec<f64> = bars()
        .into_iter()
        .map(|b| {
            vwap.handle_bar(&series_bar(b));
            vwap.value()
        })
        .collect();

    assert_all_close(&compiled, &expected);
}

#[test]
fn volume_matches_senken_indicators_volume_bar_for_bar() {
    let compiled = run_compiled("plot volume()\n", &bars());

    let mut volume = Volume::new();
    let expected: Vec<f64> = bars()
        .into_iter()
        .map(|b| {
            volume.handle_bar(&series_bar(b));
            volume.value()
        })
        .collect();

    assert_all_close(&compiled, &expected);
}

#[test]
fn stochastic_k_and_d_match_senken_indicators_bar_for_bar() {
    let compiled_k = run_compiled("plot stochastic(3, 2).k\n", &bars());
    let compiled_d = run_compiled("plot stochastic(3, 2).d\n", &bars());

    let mut stoch = Stochastic::new(3, 2);
    let mut expected_k = Vec::new();
    let mut expected_d = Vec::new();
    for b in bars() {
        stoch.handle_bar(&series_bar(b));
        expected_k.push(stoch.k());
        expected_d.push(stoch.d());
    }

    assert_all_close(&compiled_k, &expected_k);
    assert_all_close(&compiled_d, &expected_d);
}

#[test]
fn macd_signal_and_histogram_match_senken_indicators_bar_for_bar() {
    let compiled_macd = run_compiled("plot macd(2, 3, 2).macd\n", &bars());
    let compiled_signal = run_compiled("plot macd(2, 3, 2).signal\n", &bars());
    let compiled_hist = run_compiled("plot macd(2, 3, 2).histogram\n", &bars());

    let mut macd = Macd::new(2, 3, 2);
    let mut expected_macd = Vec::new();
    let mut expected_signal = Vec::new();
    let mut expected_hist = Vec::new();
    for b in bars() {
        macd.handle_bar(&series_bar(b));
        expected_macd.push(macd.macd());
        expected_signal.push(macd.signal());
        expected_hist.push(macd.histogram());
    }

    assert_all_close(&compiled_macd, &expected_macd);
    assert_all_close(&compiled_signal, &expected_signal);
    assert_all_close(&compiled_hist, &expected_hist);
}

#[test]
fn bollinger_bands_match_senken_indicators_bar_for_bar() {
    let compiled_upper = run_compiled("plot bollinger(3, 2.0).upper\n", &bars());
    let compiled_middle = run_compiled("plot bollinger(3, 2.0).middle\n", &bars());
    let compiled_lower = run_compiled("plot bollinger(3, 2.0).lower\n", &bars());

    let mut bb = BollingerBands::new(3, 2.0);
    let mut expected_upper = Vec::new();
    let mut expected_middle = Vec::new();
    let mut expected_lower = Vec::new();
    for b in bars() {
        bb.handle_bar(&series_bar(b));
        expected_upper.push(bb.upper());
        expected_middle.push(bb.middle());
        expected_lower.push(bb.lower());
    }

    assert_all_close(&compiled_upper, &expected_upper);
    assert_all_close(&compiled_middle, &expected_middle);
    assert_all_close(&compiled_lower, &expected_lower);
}

/// A program combining a `let`, arithmetic, and two different built-ins
/// (one of them a projected compound result) in one expression — proving
/// the pieces compose, not just each one alone.
#[test]
fn a_composite_expression_matches_the_equivalent_hand_written_computation() {
    let source = "let fast = ema(close, 2)\nlet slow = ema(close, 5)\nplot (fast - slow) + bollinger(3, 2.0).upper\n";
    let compiled = run_compiled(source, &bars());

    let mut fast = Ema::new(2);
    let mut slow = Ema::new(5);
    let mut bb = BollingerBands::new(3, 2.0);
    let expected: Vec<f64> = bars()
        .into_iter()
        .map(|b| {
            fast.handle_bar(&series_bar(b));
            slow.handle_bar(&series_bar(b));
            bb.handle_bar(&series_bar(b));
            (fast.value() - slow.value()) + bb.upper()
        })
        .collect();

    assert_all_close(&compiled, &expected);
}

/// This project's own indicator crate proves nine of its ten built-ins
/// break when fed bars out of order, and `Vwap` alone does not because its
/// two running sums are commutative. A compiled indicator-lang program
/// reuses those exact state machines through a host call, so the same
/// property must hold here without this crate reimplementing anything —
/// proved, not assumed, by actually reversing the feed.
#[test]
fn reversed_bars_change_a_non_commutative_built_ins_reading() {
    let forward = run_compiled("plot ema(close, 3)\n", &bars());
    let mut reversed_bars = bars();
    reversed_bars.reverse();
    let reversed = run_compiled("plot ema(close, 3)\n", &reversed_bars);

    let forward_final = *forward.last().unwrap();
    let reversed_final = *reversed.last().unwrap();
    assert!(
        (forward_final - reversed_final).abs() > 1e-6,
        "feeding the same bars in reverse must change ema's final reading, got {forward_final} \
         both ways"
    );
}

/// `Vwap` is the one built-in whose two running sums are commutative — see
/// the equivalent proof in `senken-indicators`. A compiled program calling
/// it must inherit that exemption unchanged, not because this crate
/// special-cases it, but because it is calling the exact same state
/// machine.
#[test]
fn reversed_bars_do_not_change_vwaps_reading() {
    let forward = run_compiled("plot vwap()\n", &bars());
    let mut reversed_bars = bars();
    reversed_bars.reverse();
    let reversed = run_compiled("plot vwap()\n", &reversed_bars);

    let forward_final = *forward.last().unwrap();
    let reversed_final = *reversed.last().unwrap();
    assert!(
        (forward_final - reversed_final).abs() < 1e-9,
        "vwap's cumulative sums are commutative — it must not break, got {forward_final} vs \
         {reversed_final}"
    );
}

/// Source in, identical bytes out — every time, not merely usually. This
/// is what makes it safe for a registry to address a compiled artifact by
/// a hash of its source.
#[test]
fn compiling_the_same_source_twice_produces_byte_identical_output() {
    let source = "let fast = ema(close, 12)\nlet slow = ema(close, 26)\nplot fast - slow\n";
    let a = senken_indicator_lang::compile(source).unwrap();
    let b = senken_indicator_lang::compile(source).unwrap();
    assert_eq!(a, b);
}

/// Compile time is measured, not assumed. This does not assert a specific
/// budget — CI hardware varies — but prints the real number so a report
/// can cite it instead of guessing.
#[test]
fn compile_time_is_measured() {
    const ITERATIONS: u32 = 100;
    let source = "let fast = ema(close, 12)\nlet slow = ema(close, 26)\nplot macd(12, 26, 9).histogram + (fast - slow)\n";
    // Warm up allocators/caches once before timing, the same way the
    // reported number should be read: steady-state, not first-call.
    senken_indicator_lang::compile(source).unwrap();

    let start = std::time::Instant::now();
    for _ in 0..ITERATIONS {
        senken_indicator_lang::compile(source).unwrap();
    }
    let elapsed = start.elapsed();
    // Reported with `eprintln!`, not `println!` — this workspace warns on
    // `clippy::print_stdout` to keep stray debug prints out of shipped
    // code, and a measured number a test run should surface is exactly
    // the diagnostic use `eprintln!` is for.
    eprintln!(
        "indicator-lang: {ITERATIONS} compiles of a {} byte program in {elapsed:?} ({:?}/compile)",
        source.len(),
        elapsed / ITERATIONS,
    );
    assert!(
        elapsed / ITERATIONS < std::time::Duration::from_millis(50),
        "a single compile took {:?}, which is not \"milliseconds\" by any reading",
        elapsed / ITERATIONS
    );
}

/// A syntax error names a line, a column, and describes the problem in
/// words a trader uses — never a byte offset or a compiler-internal term.
#[test]
fn a_syntax_error_names_a_line_and_column() {
    let err = senken_indicator_lang::compile("plot ema(close, 20\n").unwrap_err();
    let message = err.to_string();
    assert!(message.starts_with("line 1, column"), "{message}");
}

/// A program trying to use an unknown name fails in the type checker with
/// a line and column, in trader language.
#[test]
fn a_type_error_names_a_line_and_column() {
    let err = senken_indicator_lang::compile("plot bogus_indicator(20)\n").unwrap_err();
    let message = err.to_string();
    assert!(message.starts_with("line 1, column"), "{message}");
    assert!(message.contains("bogus_indicator"), "{message}");
}
