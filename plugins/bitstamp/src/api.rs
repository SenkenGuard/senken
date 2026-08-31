//! The subset of Bitstamp's `GET /api/v2/markets/` this crate reads.
//!
//! One bare array carries spot pairs and perpetuals. Bitstamp calls the
//! quote leg `counter_currency` and reports both increments as decimal
//! **place counts**, not sizes. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct RawMarket {
    /// Lower-case id: `btcusd`, `btcusd-perp`.
    pub(crate) market_symbol: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) base_currency: String,
    /// The quote leg, by another name.
    #[serde(default)]
    pub(crate) counter_currency: String,
    /// Decimal places in a quantity.
    #[serde(default)]
    pub(crate) base_decimals: Num,
    /// Decimal places in a price.
    #[serde(default)]
    pub(crate) counter_decimals: Num,
    /// `"Enabled"` or `"Disabled"` — a string, not a bool.
    #[serde(default)]
    pub(crate) trading: String,
    /// `SPOT` or `PERPETUAL`.
    #[serde(default)]
    pub(crate) market_type: String,
    /// Perpetuals only.
    #[serde(default)]
    pub(crate) contract_size: Num,
}
