//! The subset of Bitget's v2 public documents this crate reads.
//!
//! Both markets share an envelope whose success code is the string
//! `"00000"`. Neither publishes a plain tick size: spot gives decimal place
//! counts, and futures give a `priceEndStep` that must be scaled by
//! `pricePlace`. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Envelope<T> {
    #[serde(default)]
    pub(crate) code: String,
    #[serde(default)]
    pub(crate) msg: String,
    #[serde(default = "Vec::new")]
    pub(crate) data: Vec<T>,
}

/// One spot symbol from `/spot/public/symbols`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawSymbol {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_coin: String,
    #[serde(default)]
    pub(crate) quote_coin: String,
    #[serde(default)]
    pub(crate) status: String,
    /// Decimal places in a price, as a string.
    #[serde(default)]
    pub(crate) price_precision: Num,
    /// Decimal places in a quantity, as a string.
    #[serde(default)]
    pub(crate) quantity_precision: Num,
}

/// One contract from `/mix/market/contracts`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawContract {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_coin: String,
    #[serde(default)]
    pub(crate) quote_coin: String,
    #[serde(default)]
    pub(crate) symbol_status: String,
    /// `perpetual` or `delivery`.
    #[serde(default)]
    pub(crate) symbol_type: String,
    /// Unix milliseconds; empty on a perpetual.
    #[serde(default)]
    pub(crate) delivery_time: Num,
    /// Decimal places in a price.
    #[serde(default)]
    pub(crate) price_place: Num,
    /// How many of the last decimal place one tick spans, so the real tick
    /// is `priceEndStep × 10^-pricePlace`.
    #[serde(default)]
    pub(crate) price_end_step: Num,
    /// The quantity step, as a decimal. Preferred over `volumePlace`, which
    /// disagrees with it on some symbols.
    #[serde(default)]
    pub(crate) size_multiplier: Num,
    /// Currencies the contract can be margined in; there is no scalar
    /// settlement field.
    #[serde(default)]
    pub(crate) support_margin_coins: Vec<String>,
}
