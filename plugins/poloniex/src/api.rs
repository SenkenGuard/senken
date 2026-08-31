//! The subset of Poloniex's public documents this crate reads.
//!
//! Spot answers with a bare array and nests its increments — as decimal
//! place counts — inside `symbolTradeLimit`. Perpetuals answer under an
//! envelope with terse abbreviated keys (`bCcy`, `tSz`, `ctVal`) and give a
//! real tick size. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

/// One spot symbol from `/markets`.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawSymbol {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_currency_name: String,
    #[serde(default)]
    pub(crate) quote_currency_name: String,
    #[serde(default)]
    pub(crate) state: String,
    #[serde(default)]
    pub(crate) symbol_trade_limit: TradeLimit,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TradeLimit {
    /// Decimal places in a price.
    #[serde(default)]
    pub(crate) price_scale: Num,
    /// Decimal places in a quantity.
    #[serde(default)]
    pub(crate) quantity_scale: Num,
}

/// The envelope perpetuals arrive in.
#[derive(Debug, Deserialize)]
pub(crate) struct InstrumentsResponse {
    #[serde(default)]
    pub(crate) code: i64,
    #[serde(default)]
    pub(crate) msg: String,
    #[serde(default)]
    pub(crate) data: Vec<RawContract>,
}

/// One perpetual from `/v3/market/allInstruments`.
#[derive(Debug, Deserialize)]
pub(crate) struct RawContract {
    pub(crate) symbol: String,
    /// Base currency.
    #[serde(default, rename = "bCcy")]
    pub(crate) base_ccy: String,
    /// Quote currency.
    #[serde(default, rename = "qCcy")]
    pub(crate) quote_ccy: String,
    /// Settlement currency.
    #[serde(default, rename = "sCcy")]
    pub(crate) settle_ccy: String,
    /// Price tick.
    #[serde(default, rename = "tSz")]
    pub(crate) tick_size: Num,
    /// Quantity step, in contracts.
    #[serde(default, rename = "lotSz")]
    pub(crate) lot_size: Num,
    /// Units of the underlying per contract.
    #[serde(default, rename = "ctVal")]
    pub(crate) contract_value: Num,
    /// `LINEAR` or `INVERSE`.
    #[serde(default, rename = "ctType")]
    pub(crate) contract_type: String,
    #[serde(default)]
    pub(crate) status: String,
}
