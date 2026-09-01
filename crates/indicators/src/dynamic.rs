//! Building one of the ten built-ins from a stored `(name, params)` pair,
//! and reading a named field back out of it.
//!
//! [`Indicator`] deliberately has no `value() -> f64` — three of the ten
//! built-ins report more than one number per bar — so a `Box<dyn
//! Indicator>` alone cannot answer "what did this indicator just compute".
//! [`ConcreteIndicator`] closes that gap with a closed enum over the ten
//! concrete types instead: [`ConcreteIndicator::read`] can match on both
//! the concrete type *and* the requested [`IndicatorField`], and refuse a
//! combination that makes no sense (an [`Sma`] has no MACD line) rather
//! than silently returning some other number.
//!
//! This contract lives here rather than beside its first caller
//! (`senken-alerts`, which named it `IndicatorSpec`/`ConcreteIndicator`)
//! because a live indicator session (`senken-subscription`) needs the exact
//! same build-and-read contract and must not depend on `senken-alerts` —
//! an alert already leases its series through `senken-subscription`, so a
//! dependency the other way round would be a cycle. `senken-alerts` now
//! reuses this module instead of owning a second copy of it.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use senken_series::Bar;

use crate::average::{Ema, MovingAverage, Sma, Wma};
use crate::indicator::Indicator;
use crate::momentum::{Macd, Rsi, Stochastic};
use crate::volatility::{Atr, BollingerBands};
use crate::volume::{Volume, Vwap};

/// Which of an indicator's own reported numbers a caller wants.
///
/// Plain `Value` covers every single-valued indicator (SMA, EMA, WMA, RSI,
/// ATR, VWAP, Volume); the rest name one of the several numbers a compound
/// indicator reports per bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum IndicatorField {
    /// The single value a single-valued indicator reports.
    Value,
    /// [`Macd::macd`] — the MACD line.
    MacdLine,
    /// [`Macd::signal`] — the signal line.
    MacdSignal,
    /// [`Macd::histogram`] — MACD minus signal.
    MacdHistogram,
    /// [`Stochastic::k`].
    StochasticK,
    /// [`Stochastic::d`].
    StochasticD,
    /// [`BollingerBands::upper`].
    BollingerUpper,
    /// [`BollingerBands::middle`].
    BollingerMiddle,
    /// [`BollingerBands::lower`].
    BollingerLower,
}

impl IndicatorField {
    /// The wire name for this field — the vocabulary every consumer that
    /// puts an indicator value on the wire (a WS frame, `POST
    /// /api/indicators/compute`'s response, an alert's stored
    /// `condition_field` column) shares, so a client only ever learns one
    /// set of field names for "which number does this indicator report".
    #[must_use]
    pub fn wire_name(self) -> &'static str {
        match self {
            Self::Value => "value",
            Self::MacdLine => "macd_line",
            Self::MacdSignal => "macd_signal",
            Self::MacdHistogram => "macd_histogram",
            Self::StochasticK => "stochastic_k",
            Self::StochasticD => "stochastic_d",
            Self::BollingerUpper => "bollinger_upper",
            Self::BollingerMiddle => "bollinger_middle",
            Self::BollingerLower => "bollinger_lower",
        }
    }
}

/// Why building a concrete indicator from a stored `(name, params)` pair
/// failed, or why an [`IndicatorField`] does not apply to the indicator it
/// was asked to read from.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DynamicIndicatorError {
    /// `name` did not match any of the ten built-ins.
    #[error("unknown indicator {0:?}")]
    UnknownIndicator(String),
    /// `params` was not valid JSON, or was valid JSON missing a field the
    /// named indicator requires.
    #[error("invalid parameters for {indicator}: {reason}")]
    InvalidParams {
        /// The indicator whose parameters could not be read.
        indicator: String,
        /// Why the parameters were rejected.
        reason: String,
    },
    /// The requested [`IndicatorField`] is not one this indicator reports
    /// (e.g. asking an [`Sma`] for [`IndicatorField::MacdLine`]).
    #[error("{indicator} does not report field {field:?}")]
    FieldNotReported {
        /// The indicator that was asked.
        indicator: &'static str,
        /// The field it does not report.
        field: IndicatorField,
    },
}

/// One of this crate's ten built-ins, already constructed from a validated
/// `(name, params)` pair.
///
/// A closed enum, not `Box<dyn Indicator>`: [`read`](Self::read) needs to
/// know the concrete type to know which fields are even meaningful to ask
/// for, and [`Indicator`] itself carries no such accessor (its own docs
/// explain why: three of the ten built-ins report more than one number per
/// bar).
#[derive(Debug, Clone)]
pub enum ConcreteIndicator {
    /// [`Sma`].
    Sma(Sma),
    /// [`Ema`].
    Ema(Ema),
    /// [`Wma`].
    Wma(Wma),
    /// [`Rsi`].
    Rsi(Rsi),
    /// [`Macd`].
    Macd(Macd),
    /// [`Stochastic`].
    Stochastic(Stochastic),
    /// [`BollingerBands`].
    BollingerBands(BollingerBands),
    /// [`Atr`].
    Atr(Atr),
    /// [`Vwap`].
    Vwap(Vwap),
    /// [`Volume`].
    Volume(Volume),
}

impl ConcreteIndicator {
    /// Builds a concrete indicator from a raw `(name, params)` pair.
    ///
    /// # Errors
    /// [`DynamicIndicatorError::UnknownIndicator`] if `name` matches none of
    /// the ten built-ins; [`DynamicIndicatorError::InvalidParams`] if
    /// `params` is not valid JSON or is missing a field the named indicator
    /// requires.
    pub fn build(name: &str, params: &str) -> Result<Self, DynamicIndicatorError> {
        let json: Value = serde_json::from_str(params).map_err(|e| invalid(name, e.to_string()))?;
        match name.to_ascii_lowercase().as_str() {
            "sma" => Ok(Self::Sma(Sma::new(period(name, &json)?))),
            "ema" => Ok(Self::Ema(Ema::new(period(name, &json)?))),
            "wma" => Ok(Self::Wma(Wma::new(period(name, &json)?))),
            "rsi" => Ok(Self::Rsi(Rsi::new(period(name, &json)?))),
            "atr" => Ok(Self::Atr(Atr::new(period(name, &json)?))),
            "vwap" => Ok(Self::Vwap(Vwap::new())),
            "volume" => Ok(Self::Volume(Volume::new())),
            "macd" => {
                let fast = field_usize(name, &json, "fast_period")?;
                let slow = field_usize(name, &json, "slow_period")?;
                let signal = field_usize(name, &json, "signal_period")?;
                Ok(Self::Macd(Macd::new(fast, slow, signal)))
            }
            "stochastic" => {
                let k = field_usize(name, &json, "k_period")?;
                let d = field_usize(name, &json, "d_period")?;
                Ok(Self::Stochastic(Stochastic::new(k, d)))
            }
            "bollingerbands" | "bollinger" | "bollinger_bands" => {
                let period = field_usize(name, &json, "period")?;
                let k = field_f64(name, &json, "k")?;
                Ok(Self::BollingerBands(BollingerBands::new(period, k)))
            }
            _ => Err(DynamicIndicatorError::UnknownIndicator(name.to_owned())),
        }
    }

    /// Feeds one bar into the wrapped indicator.
    pub fn handle_bar(&mut self, bar: &Bar) {
        match self {
            Self::Sma(i) => i.handle_bar(bar),
            Self::Ema(i) => i.handle_bar(bar),
            Self::Wma(i) => i.handle_bar(bar),
            Self::Rsi(i) => i.handle_bar(bar),
            Self::Macd(i) => i.handle_bar(bar),
            Self::Stochastic(i) => i.handle_bar(bar),
            Self::BollingerBands(i) => i.handle_bar(bar),
            Self::Atr(i) => i.handle_bar(bar),
            Self::Vwap(i) => i.handle_bar(bar),
            Self::Volume(i) => i.handle_bar(bar),
        }
    }

    /// Whether the wrapped indicator has seen enough bars for its value(s)
    /// to be meaningful — a caller must never read a field while this is
    /// `false`.
    #[must_use]
    pub fn initialized(&self) -> bool {
        match self {
            Self::Sma(i) => i.initialized(),
            Self::Ema(i) => i.initialized(),
            Self::Wma(i) => i.initialized(),
            Self::Rsi(i) => i.initialized(),
            Self::Macd(i) => i.initialized(),
            Self::Stochastic(i) => i.initialized(),
            Self::BollingerBands(i) => i.initialized(),
            Self::Atr(i) => i.initialized(),
            Self::Vwap(i) => i.initialized(),
            Self::Volume(i) => i.initialized(),
        }
    }

    /// Returns the wrapped indicator to the state it was in immediately
    /// after construction — the mechanism a live session's `rebase` uses to
    /// replay a longer history without rebuilding from a stored `(name,
    /// params)` pair a second time.
    pub fn reset(&mut self) {
        match self {
            Self::Sma(i) => i.reset(),
            Self::Ema(i) => i.reset(),
            Self::Wma(i) => i.reset(),
            Self::Rsi(i) => i.reset(),
            Self::Macd(i) => i.reset(),
            Self::Stochastic(i) => i.reset(),
            Self::BollingerBands(i) => i.reset(),
            Self::Atr(i) => i.reset(),
            Self::Vwap(i) => i.reset(),
            Self::Volume(i) => i.reset(),
        }
    }

    /// Clones this indicator's current state for a provisional read.
    ///
    /// The returned indicator is independent: advancing it (via
    /// [`handle_bar`](Self::handle_bar)) must never alter the confirmed
    /// state held by the caller — the same contract
    /// [`Indicator::snapshot`] gives each concrete type, specialised here
    /// so a caller does not need to reach through `Box<dyn Indicator>`
    /// (which cannot be read back by field) to get one.
    #[must_use]
    pub fn snapshot(&self) -> Self {
        self.clone()
    }

    /// This indicator's own short name, for
    /// [`DynamicIndicatorError::FieldNotReported`].
    fn type_name(&self) -> &'static str {
        match self {
            Self::Sma(_) => "Sma",
            Self::Ema(_) => "Ema",
            Self::Wma(_) => "Wma",
            Self::Rsi(_) => "Rsi",
            Self::Macd(_) => "Macd",
            Self::Stochastic(_) => "Stochastic",
            Self::BollingerBands(_) => "BollingerBands",
            Self::Atr(_) => "Atr",
            Self::Vwap(_) => "Vwap",
            Self::Volume(_) => "Volume",
        }
    }

    /// Which [`IndicatorField`]s this indicator actually reports — an
    /// exhaustive match (not a name lookup), so a new variant added to
    /// either enum is a compile error here, not a silently incomplete
    /// catalogue or display list.
    #[must_use]
    pub fn reported_fields(&self) -> &'static [IndicatorField] {
        use IndicatorField as F;
        match self {
            Self::Sma(_)
            | Self::Ema(_)
            | Self::Wma(_)
            | Self::Rsi(_)
            | Self::Atr(_)
            | Self::Vwap(_)
            | Self::Volume(_) => &[F::Value],
            Self::Macd(_) => &[F::MacdLine, F::MacdSignal, F::MacdHistogram],
            Self::Stochastic(_) => &[F::StochasticK, F::StochasticD],
            Self::BollingerBands(_) => &[F::BollingerUpper, F::BollingerMiddle, F::BollingerLower],
        }
    }

    /// Reads `field` back out of the wrapped indicator.
    ///
    /// # Errors
    /// [`DynamicIndicatorError::FieldNotReported`] if `field` is not one
    /// this indicator's concrete type reports (e.g.
    /// [`IndicatorField::MacdLine`] asked of an [`Sma`]).
    pub fn read(&self, field: IndicatorField) -> Result<f64, DynamicIndicatorError> {
        let not_reported = || DynamicIndicatorError::FieldNotReported {
            indicator: self.type_name(),
            field,
        };
        match (self, field) {
            (Self::Sma(i), IndicatorField::Value) => Ok(i.value()),
            (Self::Ema(i), IndicatorField::Value) => Ok(i.value()),
            (Self::Wma(i), IndicatorField::Value) => Ok(i.value()),
            (Self::Rsi(i), IndicatorField::Value) => Ok(i.value()),
            (Self::Atr(i), IndicatorField::Value) => Ok(i.value()),
            (Self::Vwap(i), IndicatorField::Value) => Ok(i.value()),
            (Self::Volume(i), IndicatorField::Value) => Ok(i.value()),
            (Self::Macd(i), IndicatorField::MacdLine) => Ok(i.macd()),
            (Self::Macd(i), IndicatorField::MacdSignal) => Ok(i.signal()),
            (Self::Macd(i), IndicatorField::MacdHistogram) => Ok(i.histogram()),
            (Self::Stochastic(i), IndicatorField::StochasticK) => Ok(i.k()),
            (Self::Stochastic(i), IndicatorField::StochasticD) => Ok(i.d()),
            (Self::BollingerBands(i), IndicatorField::BollingerUpper) => Ok(i.upper()),
            (Self::BollingerBands(i), IndicatorField::BollingerMiddle) => Ok(i.middle()),
            (Self::BollingerBands(i), IndicatorField::BollingerLower) => Ok(i.lower()),
            _ => Err(not_reported()),
        }
    }
}

fn invalid(indicator: &str, reason: String) -> DynamicIndicatorError {
    DynamicIndicatorError::InvalidParams {
        indicator: indicator.to_owned(),
        reason,
    }
}

/// Reads the common `{"period": N}` shape most single-valued indicators
/// take.
fn period(indicator: &str, json: &Value) -> Result<usize, DynamicIndicatorError> {
    field_usize(indicator, json, "period")
}

fn field_usize(indicator: &str, json: &Value, field: &str) -> Result<usize, DynamicIndicatorError> {
    json.get(field)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| {
            invalid(
                indicator,
                format!("missing or invalid `{field}` (expected a non-negative integer)"),
            )
        })
}

fn field_f64(indicator: &str, json: &Value, field: &str) -> Result<f64, DynamicIndicatorError> {
    json.get(field).and_then(Value::as_f64).ok_or_else(|| {
        invalid(
            indicator,
            format!("missing or invalid `{field}` (expected a number)"),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::{ConcreteIndicator, IndicatorField};

    #[test]
    fn unknown_indicator_name_is_rejected() {
        let err = ConcreteIndicator::build("NotARealIndicator", "{}").unwrap_err();
        assert!(matches!(
            err,
            super::DynamicIndicatorError::UnknownIndicator(name) if name == "NotARealIndicator"
        ));
    }

    #[test]
    fn missing_period_is_rejected_with_the_indicators_own_name() {
        let err = ConcreteIndicator::build("Sma", "{}").unwrap_err();
        assert!(matches!(
            err,
            super::DynamicIndicatorError::InvalidParams { indicator, .. } if indicator == "Sma"
        ));
    }

    #[test]
    fn malformed_json_is_rejected_not_panicked() {
        let err = ConcreteIndicator::build("Sma", "not json").unwrap_err();
        assert!(matches!(
            err,
            super::DynamicIndicatorError::InvalidParams { .. }
        ));
    }

    #[test]
    fn building_every_one_of_the_ten_built_ins_succeeds_with_valid_params() {
        for (name, params) in [
            ("Sma", r#"{"period":5}"#),
            ("Ema", r#"{"period":5}"#),
            ("Wma", r#"{"period":5}"#),
            ("Rsi", r#"{"period":14}"#),
            ("Atr", r#"{"period":14}"#),
            ("Vwap", "{}"),
            ("Volume", "{}"),
            (
                "Macd",
                r#"{"fast_period":12,"slow_period":26,"signal_period":9}"#,
            ),
            ("Stochastic", r#"{"k_period":14,"d_period":3}"#),
            ("BollingerBands", r#"{"period":20,"k":2.0}"#),
        ] {
            ConcreteIndicator::build(name, params).unwrap_or_else(|e| panic!("{name}: {e}"));
        }
    }

    #[test]
    fn asking_a_single_valued_indicator_for_a_macd_field_is_refused() {
        let sma = ConcreteIndicator::build("Sma", r#"{"period":3}"#).unwrap();
        let err = sma.read(IndicatorField::MacdLine).unwrap_err();
        assert!(matches!(
            err,
            super::DynamicIndicatorError::FieldNotReported {
                indicator: "Sma",
                field: IndicatorField::MacdLine
            }
        ));
    }

    #[test]
    fn indicator_name_matching_is_case_insensitive() {
        ConcreteIndicator::build("sMa", r#"{"period":5}"#).unwrap();
        ConcreteIndicator::build(
            "MACD",
            r#"{"fast_period":12,"slow_period":26,"signal_period":9}"#,
        )
        .unwrap();
    }

    /// The dynamic-read contract's own proof that a snapshot cannot affect
    /// the indicator it was taken from — the same property
    /// `average::ema`'s own test proves for one concrete type, reproduced
    /// here at the `ConcreteIndicator` level since this is the shape a live
    /// session actually holds.
    #[test]
    fn snapshotting_a_concrete_indicator_does_not_advance_the_original() {
        use crate::test_support::{assert_approx_eq, bar};

        let mut sma = ConcreteIndicator::build("Sma", r#"{"period":1}"#).unwrap();
        sma.handle_bar(&bar(0, 0, 0, 10, 0));

        let mut provisional = sma.snapshot();
        provisional.handle_bar(&bar(0, 0, 0, 20, 0));

        assert_approx_eq(sma.read(IndicatorField::Value).unwrap(), 10.0);
        assert_approx_eq(provisional.read(IndicatorField::Value).unwrap(), 20.0);
    }

    #[test]
    fn reported_fields_matches_what_read_actually_accepts() {
        let macd = ConcreteIndicator::build(
            "Macd",
            r#"{"fast_period":2,"slow_period":3,"signal_period":2}"#,
        )
        .unwrap();
        for field in macd.reported_fields() {
            // Warm-up aside, `read` must not refuse a field this indicator
            // itself claims to report.
            assert!(!matches!(
                macd.read(*field),
                Err(super::DynamicIndicatorError::FieldNotReported { .. })
            ));
        }
    }
}
