//! The subset of Gate.io's v4 public documents this crate reads.
//!
//! Gate answers with bare top-level arrays and no envelope; a failure is a
//! non-2xx status carrying `{label, message}`. Spot reports precision as
//! digit counts, derivatives as decimal strings. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

/// One spot pair from `/spot/currency_pairs`.
#[derive(Debug, Deserialize)]
pub(crate) struct RawPair {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) base: String,
    #[serde(default)]
    pub(crate) quote: String,
    #[serde(default)]
    pub(crate) trade_status: String,
    /// Decimal places in a price.
    #[serde(default)]
    pub(crate) precision: Num,
    /// Decimal places in a quantity.
    #[serde(default)]
    pub(crate) amount_precision: Num,
}

/// One contract from `/futures/{settle}/contracts` or
/// `/delivery/{settle}/contracts`.
#[derive(Debug, Deserialize)]
pub(crate) struct RawContract {
    /// `BTC_USDT`; there are no separate base and quote fields.
    pub(crate) name: String,
    /// `direct` (linear) or `inverse`. Not `contract_type`, which is an
    /// asset-class tag such as `stocks` or `forex`.
    #[serde(default, rename = "type")]
    pub(crate) contract_kind: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) in_delisting: bool,
    #[serde(default)]
    pub(crate) order_price_round: Num,
    /// Units of the underlying per contract.
    #[serde(default)]
    pub(crate) quanto_multiplier: Num,
    /// Expiry in Unix **seconds**, on delivery contracts only.
    #[serde(default)]
    pub(crate) expire_time: Num,
}
