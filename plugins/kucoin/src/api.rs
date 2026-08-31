//! The subset of KuCoin's public documents this crate reads.
//!
//! Spot and futures live on separate hosts and share only their envelope.
//! Spot sends decimal strings; futures send JSON numbers for the same
//! concepts, which [`Num`] absorbs. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

/// The envelope both hosts wrap their payload in.
#[derive(Debug, Deserialize)]
pub(crate) struct Envelope<T> {
    #[serde(default)]
    pub(crate) code: String,
    #[serde(default)]
    pub(crate) msg: String,
    #[serde(default = "Vec::new")]
    pub(crate) data: Vec<T>,
}

/// One spot symbol from `api.kucoin.com`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawSymbol {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_currency: String,
    #[serde(default)]
    pub(crate) quote_currency: String,
    #[serde(default)]
    pub(crate) enable_trading: bool,
    #[serde(default)]
    pub(crate) price_increment: Num,
    #[serde(default)]
    pub(crate) base_increment: Num,
}

/// One contract from `api-futures.kucoin.com`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawContract {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_currency: String,
    #[serde(default)]
    pub(crate) quote_currency: String,
    #[serde(default)]
    pub(crate) settle_currency: String,
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) tick_size: Num,
    /// Quantity step, in contracts.
    #[serde(default)]
    pub(crate) lot_size: Num,
    /// Units per contract. **Negative on inverse contracts**, where the
    /// sign is a second encoding of `isInverse`.
    #[serde(default)]
    pub(crate) multiplier: Num,
    #[serde(default)]
    pub(crate) is_inverse: bool,
    /// Unix milliseconds; `null` on a perpetual.
    #[serde(default)]
    pub(crate) expire_date: Option<i64>,
}
