//! [`IndicatorSpec`] — a stored `(name, params)` pair naming one of
//! `senken-indicators`' ten built-ins.
//!
//! The dynamic build-and-read contract itself
//! ([`senken_indicators::ConcreteIndicator`], re-exported here as
//! [`ConcreteIndicator`]) moved to `senken-indicators`: a live indicator
//! session (`senken-subscription`) needs the exact same "build one of the
//! ten built-ins from a name and read a value back out of it" contract, and
//! must not depend on this crate to get it (an alert already leases its
//! series through `senken-subscription`, so the reverse dependency would be
//! a cycle). This crate keeps only the thin, alert-specific wrapper:
//! [`IndicatorSpec`] is the stored `(name, params)` pair persisted in an
//! alert row.

pub use senken_indicators::ConcreteIndicator;
use serde::{Deserialize, Serialize};

use crate::error::IndicatorSpecError;

/// A stored, opaque `(name, params)` pair — the middle element of an
/// alert's `(series key, indicator spec, condition)` triple.
/// Mirrors how `senken-chart`'s `ItemSource::Computed` stores a computed
/// pane item, since both are ultimately naming one of the same ten
/// built-ins.
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

#[cfg(test)]
mod tests {
    use super::IndicatorSpec;

    fn spec(name: &str, params: &str) -> IndicatorSpec {
        IndicatorSpec {
            name: name.to_owned(),
            params: params.to_owned(),
        }
    }

    /// `senken-indicators` proves `ConcreteIndicator::build` exhaustively
    /// (all ten built-ins, case-insensitivity, malformed JSON, unknown
    /// names, field-not-reported). This crate's own responsibility is only
    /// that [`IndicatorSpec::build`] actually delegates there rather than
    /// silently duplicating (or drifting from) that logic.
    #[test]
    fn build_delegates_to_the_dynamic_indicator_contract() {
        spec("Sma", r#"{"period":5}"#)
            .build()
            .expect("a valid spec must build");

        let err = spec("NotARealIndicator", "{}").build().unwrap_err();
        assert!(matches!(
            err,
            super::IndicatorSpecError::UnknownIndicator(name) if name == "NotARealIndicator"
        ));
    }
}
