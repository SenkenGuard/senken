//! The subset of WhiteBIT's `GET /api/v4/public/markets` this crate reads.
//!
//! One bare array carries spot pairs and perpetuals, told apart by `type`.
//! WhiteBIT names the two legs `stock` and `money` rather than base and
//! quote. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawMarket {
    pub(crate) name: String,
    /// The base asset.
    #[serde(default)]
    pub(crate) stock: String,
    /// The quote asset.
    #[serde(default)]
    pub(crate) money: String,
    /// `spot` or `futures`.
    #[serde(default, rename = "type")]
    pub(crate) market_type: String,
    #[serde(default)]
    pub(crate) trades_enabled: bool,
    /// Set once a market has been delisted.
    #[serde(default)]
    pub(crate) delisted_at: Option<serde_json::Value>,
    #[serde(default)]
    pub(crate) tick_size: Num,
    #[serde(default)]
    pub(crate) step_size: Num,
}
