//! [`IndicatorSpec`] and [`ConcreteIndicator`] — wiring a stored `(name,
//! params)` pair to one of `senken-indicators`' ten built-ins,
//! the piece left until now ("wiring indicators into
//! alerts").
//!
//! [`senken_indicators::Indicator`] deliberately has no `value() -> f64`
//! (three of the ten built-ins report more than one number per bar), so a
//! `Box<dyn Indicator>` alone is not enough to read a value back out —
//! [`ConcreteIndicator`] is a closed enum over the ten concrete types
//! instead, so [`ConcreteIndicator::read`] can match on both the concrete
//! type *and* the requested [`crate::condition::IndicatorField`] and refuse
//! a combination that makes no sense (an `Sma` has no MACD line) rather than
//! silently returning some other number.

use senken_indicators::{
    Atr, BollingerBands, Ema, Indicator, Macd, MovingAverage, Rsi, Sma, Stochastic, Volume, Vwap,
    Wma,
};
use senken_series::Bar;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::condition::IndicatorField;
use crate::error::IndicatorSpecError;

/// A stored, opaque `(name, params)` pair — the middle element of an
/// alert's `(series key, indicator spec, condition)` triple.
/// Mirrors how `senken-workspace`'s `LayerKind::IndicatorOverlay`/
/// `IndicatorSubPane` store an indicator layer, since both are ultimately
/// naming one of the same ten built-ins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndicatorSpec {
    /// The indicator's name, matched case-insensitively against the ten
    /// built-ins — `"Sma"`, `"Ema"`, `"Wma"`, `"Rsi"`,
    /// `"Macd"`, `"Stochastic"`, `"BollingerBands"`, `"Atr"`, `"Vwap"`,
    /// `"Volume"`.
    pub name: String,
    /// The indicator's parameters, as a JSON object — see
    /// [`ConcreteIndicator::build`] for each indicator's required fields.
    pub params: String,
}

impl IndicatorSpec {
    /// Builds the concrete indicator this spec names.
    ///
    /// # Errors
    /// [`IndicatorSpecError::UnknownIndicator`] if `name` matches none of
    /// the ten built-ins; [`IndicatorSpecError::InvalidParams`] if `params`
    /// is not valid JSON or is missing a field the named indicator
    /// requires.
    pub fn build(&self) -> Result<ConcreteIndicator, IndicatorSpecError> {
        ConcreteIndicator::build(&self.name, &self.params)
    }
}

/// One of `senken-indicators`' ten built-ins, already
/// constructed from a validated [`IndicatorSpec`].
///
/// A closed enum, not `Box<dyn Indicator>`: [`read`](Self::read) needs to
/// know the concrete type to know which fields are even meaningful to ask
/// for, and `Indicator` itself carries no such accessor (the /// docs explain why: three of the ten built-ins report more than one number
/// per bar).
#[derive(Debug)]
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
    /// Builds a concrete indicator from a raw `(name, params)` pair — see
    /// [`IndicatorSpec::build`], which this backs.
    ///
    /// # Errors
    /// [`IndicatorSpecError::UnknownIndicator`] if `name` matches none of
    /// the ten built-ins; [`IndicatorSpecError::InvalidParams`] if `params`
    /// is not valid JSON or is missing a field the named indicator
    /// requires.
    pub fn build(name: &str, params: &str) -> Result<Self, IndicatorSpecError> {
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
            _ => Err(IndicatorSpecError::UnknownIndicator(name.to_owned())),
        }
    }

    /// Feeds one closed bar into the wrapped indicator.
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
    /// to be meaningful — an alert must never fire while this
    /// is `false`.
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

    /// This indicator's own short name, for [`IndicatorSpecError::FieldNotReported`].
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

    /// Reads `field` back out of the wrapped indicator.
    ///
    /// # Errors
    /// [`IndicatorSpecError::FieldNotReported`] if `field` is not one this
    /// indicator's concrete type reports (e.g. [`IndicatorField::MacdLine`]
    /// asked of an [`Sma`]).
    pub fn read(&self, field: IndicatorField) -> Result<f64, IndicatorSpecError> {
        let not_reported = || IndicatorSpecError::FieldNotReported {
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

fn invalid(indicator: &str, reason: String) -> IndicatorSpecError {
    IndicatorSpecError::InvalidParams {
        indicator: indicator.to_owned(),
        reason,
    }
}

/// Reads the common `{"period": N}` shape most single-valued indicators
/// take.
fn period(indicator: &str, json: &Value) -> Result<usize, IndicatorSpecError> {
    field_usize(indicator, json, "period")
}

fn field_usize(indicator: &str, json: &Value, field: &str) -> Result<usize, IndicatorSpecError> {
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

fn field_f64(indicator: &str, json: &Value, field: &str) -> Result<f64, IndicatorSpecError> {
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
            super::IndicatorSpecError::UnknownIndicator(name) if name == "NotARealIndicator"
        ));
    }

    #[test]
    fn missing_period_is_rejected_with_the_indicators_own_name() {
        let err = ConcreteIndicator::build("Sma", "{}").unwrap_err();
        assert!(matches!(
            err,
            super::IndicatorSpecError::InvalidParams { indicator, .. } if indicator == "Sma"
        ));
    }

    #[test]
    fn malformed_json_is_rejected_not_panicked() {
        let err = ConcreteIndicator::build("Sma", "not json").unwrap_err();
        assert!(matches!(
            err,
            super::IndicatorSpecError::InvalidParams { .. }
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
            super::IndicatorSpecError::FieldNotReported {
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
}
