//! Backs `wit/senken.wit`'s `builtins` import with the real
//! `senken_indicators` types, so a plugin calling `ema(close, 20)` calls
//! the same compiled, already-tested `Ema` this application uses
//! everywhere else — never a second implementation living inside a guest.
//!
//! This is the reason this crate — the layer that actually loads and runs
//! a plugin — is allowed to depend on a domain crate at all: `senken-
//! plugin-api` cannot (a published SDK must never publish a domain crate's
//! implementation alongside it), but nothing stops the host that mediates
//! every call a plugin makes back into Senken.

use std::collections::HashMap;

use senken_core::UnixNanos;
use senken_indicators::{
    Atr, BollingerBands, Ema, Indicator, Macd, MovingAverage, Rsi, Sma, Stochastic, Volume, Vwap,
    Wma,
};
use senken_series::Bar;
use wasmtime::component::{HasSelf, Linker};

use crate::bindings::generated::senken::plugin_api::builtins::Host;
use crate::wasi::PluginState;

/// One instance per call-site `slot`, exactly as `wit/senken.wit`'s
/// `builtins` interface documents: constructed lazily on that slot's first
/// call, kept for as long as the plugin instance that owns it runs.
///
/// Lives on [`PluginState`], not on this crate's `Linker` — the `Linker` is
/// shared across every plugin this host ever loads, but one plugin's EMA
/// must never see another plugin's bars.
#[derive(Debug, Default)]
pub(crate) struct BuiltinState {
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
}

impl Host for PluginState {
    fn sma_update(&mut self, slot_id: u32, value: f64, period: u32) -> f64 {
        let sma = self
            .builtins
            .sma
            .entry(slot_id)
            .or_insert_with(|| Sma::new(period as usize));
        sma.update_raw(value);
        sma.value()
    }

    fn ema_update(&mut self, slot_id: u32, value: f64, period: u32) -> f64 {
        let ema = self
            .builtins
            .ema
            .entry(slot_id)
            .or_insert_with(|| Ema::new(period as usize));
        ema.update_raw(value);
        ema.value()
    }

    fn wma_update(&mut self, slot_id: u32, value: f64, period: u32) -> f64 {
        let wma = self
            .builtins
            .wma
            .entry(slot_id)
            .or_insert_with(|| Wma::new(period as usize));
        wma.update_raw(value);
        wma.value()
    }

    fn rsi_update(&mut self, slot_id: u32, period: u32, close: f64) -> f64 {
        let rsi = self
            .builtins
            .rsi
            .entry(slot_id)
            .or_insert_with(|| Rsi::new(period as usize));
        rsi.handle_bar(&bar_close_only(close));
        rsi.value()
    }

    fn atr_update(&mut self, slot_id: u32, period: u32, high: f64, low: f64, close: f64) -> f64 {
        let atr = self
            .builtins
            .atr
            .entry(slot_id)
            .or_insert_with(|| Atr::new(period as usize));
        atr.handle_bar(&bar_hlc(high, low, close));
        atr.value()
    }

    fn vwap_update(&mut self, slot_id: u32, high: f64, low: f64, close: f64, volume: f64) -> f64 {
        let vwap = self.builtins.vwap.entry(slot_id).or_default();
        vwap.handle_bar(&bar_full(high, low, close, volume));
        vwap.value()
    }

    fn volume_update(&mut self, slot_id: u32, volume: f64) -> f64 {
        let vol = self.builtins.volume.entry(slot_id).or_default();
        vol.handle_bar(&bar_volume_only(volume));
        vol.value()
    }

    fn stochastic_update(
        &mut self,
        slot_id: u32,
        k_period: u32,
        d_period: u32,
        high: f64,
        low: f64,
        close: f64,
    ) -> (f64, f64) {
        let stoch = self
            .builtins
            .stochastic
            .entry(slot_id)
            .or_insert_with(|| Stochastic::new(k_period as usize, d_period as usize));
        stoch.handle_bar(&bar_hlc(high, low, close));
        (stoch.k(), stoch.d())
    }

    fn macd_update(
        &mut self,
        slot_id: u32,
        fast_period: u32,
        slow_period: u32,
        signal_period: u32,
        close: f64,
    ) -> (f64, f64, f64) {
        let macd = self.builtins.macd.entry(slot_id).or_insert_with(|| {
            Macd::new(
                fast_period as usize,
                slow_period as usize,
                signal_period as usize,
            )
        });
        macd.handle_bar(&bar_close_only(close));
        (macd.macd(), macd.signal(), macd.histogram())
    }

    fn bollinger_update(
        &mut self,
        slot_id: u32,
        period: u32,
        k: f64,
        close: f64,
    ) -> (f64, f64, f64) {
        let bb = self
            .builtins
            .bollinger
            .entry(slot_id)
            .or_insert_with(|| BollingerBands::new(period as usize, k));
        bb.handle_bar(&bar_close_only(close));
        (bb.upper(), bb.middle(), bb.lower())
    }
}

/// Adds this host's implementation of the `builtins` import to `linker`.
/// Call once per [`wasmtime::Engine`] — the same linker instantiates every
/// plugin loaded through it, and each plugin's own [`PluginState`] (not
/// this call) is what actually separates one plugin's built-in state from
/// another's.
///
/// # Errors
/// Only if `wasmtime`'s own binding generation rejects a duplicate
/// registration, which does not happen when this runs once per linker.
pub(crate) fn add_to_linker(linker: &mut Linker<PluginState>) -> wasmtime::Result<()> {
    crate::bindings::generated::senken::plugin_api::builtins::add_to_linker::<
        PluginState,
        HasSelf<PluginState>,
    >(linker, |state| state)
}

/// A bar carrying only `close`, for the built-ins whose `handle_bar` reads
/// nothing else (`rsi`, `macd`, `bollinger`).
fn bar_close_only(close: f64) -> Bar {
    let close = bar_field_from_f64(close);
    Bar {
        ts_open: UnixNanos::EPOCH,
        open: close,
        high: close,
        low: close,
        close,
        volume: senken_series::Volume::Absent,
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// A bar carrying `high`/`low`/`close`, for `atr` and `stochastic`.
fn bar_hlc(high: f64, low: f64, close: f64) -> Bar {
    Bar {
        ts_open: UnixNanos::EPOCH,
        open: bar_field_from_f64(close),
        high: bar_field_from_f64(high),
        low: bar_field_from_f64(low),
        close: bar_field_from_f64(close),
        volume: senken_series::Volume::Absent,
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// A bar carrying every field `vwap` reads.
fn bar_full(high: f64, low: f64, close: f64, volume: f64) -> Bar {
    Bar {
        ts_open: UnixNanos::EPOCH,
        open: bar_field_from_f64(close),
        high: bar_field_from_f64(high),
        low: bar_field_from_f64(low),
        close: bar_field_from_f64(close),
        volume: senken_series::Volume::Real(bar_field_from_f64(volume)),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// A bar carrying only `volume`, for the `volume` built-in.
fn bar_volume_only(volume: f64) -> Bar {
    Bar {
        ts_open: UnixNanos::EPOCH,
        open: 0,
        high: 0,
        low: 0,
        close: 0,
        volume: senken_series::Volume::Real(bar_field_from_f64(volume)),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    }
}

/// Converts a wasm-supplied `f64` bar field back to the `i64`
/// [`senken_series::Bar`] needs.
///
/// `wit/senken.wit`'s `builtins` interface carries every price and
/// quantity as a plain `f64` — the same "indicator values may be `f64`"
/// exception `senken_indicators` itself documents, not the "never `f64`
/// for money" rule, since these values never reach anything that trades.
/// A real caller's `f64` here is a `senken_core::Scaled::value` widened
/// through `as f64` (lossless up to 2^53, the same headroom
/// `senken_indicators::convert::scaled_to_f64` already accepts for the
/// return trip), so it is always a whole number in practice — but nothing
/// at this WIT boundary *proves* that to the compiler, and a guest is an
/// untrusted caller, not this crate's own test fixture.
///
/// A bare `as` cast would answer the out-of-range and non-finite cases the
/// same way this does (`as f64 -> i64` has saturated, not wrapped, since
/// Rust 1.45), but would do it invisibly and trip
/// `clippy::cast_possible_truncation` besides. Spelling every case out
/// with [`num_traits::cast`] keeps that exact, already-defined behaviour
/// while making it something this crate states rather than inherits
/// silently from the cast operator.
fn bar_field_from_f64(value: f64) -> i64 {
    if value.is_nan() {
        0
    } else if let Some(exact) = num_traits::cast::<f64, i64>(value) {
        exact
    } else if value.is_sign_negative() {
        i64::MIN
    } else {
        i64::MAX
    }
}
