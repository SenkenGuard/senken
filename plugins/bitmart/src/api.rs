//! The subset of BitMart's public documents this crate reads.
//!
//! Spot and futures live on different hosts and share only their envelope
//! shape. Spot reports the price as a decimal place count and the quantity
//! as an increment; futures report both as decimals. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Envelope<T> {
    #[serde(default)]
    pub(crate) code: i64,
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) data: T,
}

#[derive(Debug, Deserialize)]
pub(crate) struct Symbols<T> {
    #[serde(default = "Vec::new")]
    pub(crate) symbols: Vec<T>,
}

// Written by hand rather than derived: deriving would demand `T: Default`,
// which the payload types have no reason to satisfy.
impl<T> Default for Symbols<T> {
    fn default() -> Self {
        Self {
            symbols: Vec::new(),
        }
    }
}

/// One spot pair from `/spot/v1/symbols/details`.
#[derive(Debug, Deserialize)]
pub(crate) struct RawSpot {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_currency: String,
    #[serde(default)]
    pub(crate) quote_currency: String,
    /// The quantity increment.
    #[serde(default)]
    pub(crate) quote_increment: Num,
    /// The **finest** number of decimal places a price may carry, and so
    /// the one that describes the tick. Its sibling `price_min_precision`
    /// is the coarsest and runs three places behind — it is `-1` on
    /// `BTC_USDT`, which as a tick would mean ten dollars.
    #[serde(default)]
    pub(crate) price_max_precision: Num,
    #[serde(default)]
    pub(crate) trade_status: String,
}

/// One contract from `/contract/public/details`.
#[derive(Debug, Deserialize)]
pub(crate) struct RawContract {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_currency: String,
    #[serde(default)]
    pub(crate) quote_currency: String,
    /// Units of the underlying per contract.
    #[serde(default)]
    pub(crate) contract_size: Num,
    /// The price tick, as a decimal string — despite the name.
    #[serde(default)]
    pub(crate) price_precision: Num,
    /// The quantity step, in contracts.
    #[serde(default)]
    pub(crate) vol_precision: Num,
    /// Unix milliseconds; `0` on a perpetual.
    #[serde(default)]
    pub(crate) expire_timestamp: Num,
    #[serde(default)]
    pub(crate) status: String,
}
