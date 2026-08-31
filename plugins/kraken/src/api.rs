//! The subset of Kraken's public documents this crate reads.
//!
//! Spot and futures are separate products with separate shapes: spot
//! answers with an **object keyed by pair name**, futures with an array.
//! Only consumed fields are declared. Inbound only.

use std::collections::HashMap;

use senken_venue::Num;
use serde::Deserialize;

/// `GET /0/public/AssetPairs`.
#[derive(Debug, Deserialize)]
pub(crate) struct AssetPairsResponse {
    /// Kraken reports failures in a body that is otherwise HTTP 200.
    #[serde(default)]
    pub(crate) error: Vec<String>,
    #[serde(default)]
    pub(crate) result: HashMap<String, RawPair>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawPair {
    /// The compact name, `XBTUSD`.
    #[serde(default)]
    pub(crate) altname: String,
    /// The display name, `XBT/USD` — the cleanest source of base and quote,
    /// since `base`/`quote` carry Kraken's legacy `X`/`Z` prefixes.
    #[serde(default)]
    pub(crate) wsname: String,
    #[serde(default)]
    pub(crate) base: String,
    #[serde(default)]
    pub(crate) quote: String,
    #[serde(default)]
    pub(crate) status: String,
    /// The price tick, as a decimal string.
    #[serde(default)]
    pub(crate) tick_size: Num,
    /// Decimal places in a quantity; there is no decimal step field.
    #[serde(default)]
    pub(crate) lot_decimals: Num,
}

/// `GET /derivatives/api/v3/instruments`.
#[derive(Debug, Deserialize)]
pub(crate) struct InstrumentsResponse {
    #[serde(default)]
    pub(crate) result: String,
    #[serde(default)]
    pub(crate) error: String,
    #[serde(default)]
    pub(crate) instruments: Vec<RawInstrument>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawInstrument {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base: String,
    #[serde(default)]
    pub(crate) quote: String,
    /// `flexible_futures` (multi-collateral) or `futures_inverse`.
    #[serde(default, rename = "type")]
    pub(crate) instrument_type: String,
    #[serde(default)]
    pub(crate) tradeable: bool,
    #[serde(default)]
    pub(crate) is_expired: bool,
    /// A JSON number, sometimes as small as `1e-11`.
    #[serde(default)]
    pub(crate) tick_size: Num,
    /// Decimal places in a quantity — **may be negative**, in which case
    /// the step is larger than one unit.
    #[serde(default)]
    pub(crate) contract_value_trade_precision: Num,
    #[serde(default)]
    pub(crate) contract_size: Num,
    /// ISO 8601, present only on dated contracts.
    #[serde(default)]
    pub(crate) last_trading_time: String,
}
