//! The subset of Coinbase's public product documents this crate reads.
//!
//! Two hosts, two shapes: Coinbase Exchange (`api.exchange.coinbase.com`)
//! lists spot products, and Coinbase International
//! (`api.international.coinbase.com`) lists perpetuals. Both answer with a
//! bare top-level array. Only consumed fields are declared. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

/// One product on Coinbase Exchange — spot only.
#[derive(Debug, Deserialize)]
pub(crate) struct RawProduct {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) base_currency: String,
    #[serde(default)]
    pub(crate) quote_currency: String,
    #[serde(default)]
    pub(crate) status: String,
    /// Set while a listed product is not accepting orders.
    #[serde(default)]
    pub(crate) trading_disabled: bool,
    /// The price tick.
    #[serde(default)]
    pub(crate) quote_increment: Num,
    /// The quantity step.
    #[serde(default)]
    pub(crate) base_increment: Num,
}

/// One instrument on Coinbase International.
#[derive(Debug, Deserialize)]
pub(crate) struct RawInstrument {
    pub(crate) symbol: String,
    /// `SPOT` or `PERP`.
    #[serde(default, rename = "type")]
    pub(crate) instrument_type: String,
    #[serde(default)]
    pub(crate) base_asset_name: String,
    #[serde(default)]
    pub(crate) quote_asset_name: String,
    #[serde(default)]
    pub(crate) trading_state: String,
    #[serde(default)]
    pub(crate) quote_increment: Num,
    #[serde(default)]
    pub(crate) base_increment: Num,
}
