//! The subset of MEXC's public documents this crate reads.
//!
//! Spot mirrors Binance's `exchangeInfo` shape but **without usable
//! filters** — its `filters` array carries only `PERCENT_PRICE_BY_SIDE`, so
//! the increments have to come from the precision fields. Futures use a
//! different envelope entirely and do give real increments. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ExchangeInfo {
    #[serde(default)]
    pub(crate) symbols: Vec<RawSymbol>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawSymbol {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_asset: String,
    #[serde(default)]
    pub(crate) quote_asset: String,
    /// Decimal places in a price.
    #[serde(default)]
    pub(crate) quote_precision: Num,
    /// Decimal places in a quantity.
    #[serde(default)]
    pub(crate) base_asset_precision: Num,
    /// `status` is the string `"1"` for every symbol, so these two bools
    /// are the only real signal of whether a pair trades.
    #[serde(default)]
    pub(crate) is_spot_trading_allowed: bool,
    /// Set while a symbol is under a trading-protection halt.
    #[serde(default)]
    pub(crate) st: bool,
}

/// The envelope futures arrive in.
#[derive(Debug, Deserialize)]
pub(crate) struct ContractDetail {
    #[serde(default)]
    pub(crate) success: bool,
    #[serde(default)]
    pub(crate) code: i64,
    #[serde(default)]
    pub(crate) data: Vec<RawContract>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawContract {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_coin: String,
    #[serde(default)]
    pub(crate) quote_coin: String,
    /// What the contract settles in. MEXC publishes no linear/inverse flag
    ///   — `futureType` is `1` for every contract — so this is the only way
    /// to tell the two apart.
    #[serde(default)]
    pub(crate) settle_coin: String,
    #[serde(default)]
    pub(crate) contract_size: Num,
    /// `0` while the contract is live.
    #[serde(default)]
    pub(crate) state: i64,
    /// The price tick, as a JSON number.
    #[serde(default)]
    pub(crate) price_unit: Num,
    /// The quantity step, in contracts.
    #[serde(default)]
    pub(crate) vol_unit: Num,
}
