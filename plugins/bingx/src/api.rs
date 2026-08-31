//! The subset of BingX's public documents this crate reads.
//!
//! Three endpoints, three shapes. None of them names the base and quote
//! separately, so every pair is split out of its symbol. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

/// The envelope every BingX endpoint uses; `code` is `0` on success.
#[derive(Debug, Deserialize)]
pub(crate) struct Envelope<T> {
    #[serde(default)]
    pub(crate) code: i64,
    #[serde(default)]
    pub(crate) msg: String,
    #[serde(default)]
    pub(crate) data: T,
}

/// Spot nests its array one level deeper than the other two.
#[derive(Debug, Default, Deserialize)]
pub(crate) struct SpotData {
    #[serde(default)]
    pub(crate) symbols: Vec<RawSpot>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawSpot {
    pub(crate) symbol: String,
    /// `1` while the pair trades.
    #[serde(default)]
    pub(crate) status: i64,
    /// A JSON number, sometimes in scientific notation.
    #[serde(default)]
    pub(crate) tick_size: Num,
    #[serde(default)]
    pub(crate) step_size: Num,
}

/// One linear perpetual from `/swap/v2/quote/contracts`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawLinear {
    pub(crate) symbol: String,
    /// The base asset.
    #[serde(default)]
    pub(crate) asset: String,
    /// The quote asset, which is also what the contract settles in.
    #[serde(default)]
    pub(crate) currency: String,
    /// Units of the underlying per contract.
    #[serde(default)]
    pub(crate) size: Num,
    #[serde(default)]
    pub(crate) status: i64,
    /// Decimal places; there is no tick size field on this market.
    #[serde(default)]
    pub(crate) price_precision: Num,
    #[serde(default)]
    pub(crate) quantity_precision: Num,
}

/// One inverse perpetual from `/cswap/v1/market/contracts`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawInverse {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) status: i64,
    /// Decimal places in a price.
    #[serde(default)]
    pub(crate) price_precision: Num,
    /// **Not a tick size.** It equals `minTradeValue`, the per-contract
    /// notional in USD, and mapping it to the price tick would be badly
    /// wrong — `BTC-USD` reports `100`.
    #[serde(default)]
    pub(crate) min_trade_value: Num,
}
