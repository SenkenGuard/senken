//! The subset of HTX's public documents this crate reads.
//!
//! Spot lives on `api.huobi.pro` and abbreviates every key to two letters
//! (`bc` base, `qc` quote, `pp` price precision); derivatives live on
//! `api.hbdm.com` with readable names but no base/quote fields at all —
//! `contract_code` carries the pair and `symbol` is only the base coin.
//! Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct Envelope<T> {
    /// `"ok"` on success.
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) err_msg: String,
    #[serde(default = "Vec::new")]
    pub(crate) data: Vec<T>,
}

/// One spot symbol from `/v1/settings/common/market-symbols`.
#[derive(Debug, Deserialize)]
pub(crate) struct RawSymbol {
    /// Lower case, concatenated: `btcusdt`.
    pub(crate) symbol: String,
    /// Base currency, lower case.
    #[serde(default)]
    pub(crate) bc: String,
    /// Quote currency, lower case.
    #[serde(default)]
    pub(crate) qc: String,
    /// `online`, `offline` or `suspend`.
    #[serde(default)]
    pub(crate) state: String,
    /// Decimal places in a price.
    #[serde(default)]
    pub(crate) pp: Num,
    /// Decimal places in an amount.
    #[serde(default)]
    pub(crate) ap: Num,
}

/// One contract from any of the three derivative endpoints.
#[derive(Debug, Deserialize)]
pub(crate) struct RawContract {
    /// The tradable id: `BTC-USDT`, `BTC-USD`, `BTC260904`.
    pub(crate) contract_code: String,
    /// The base coin only — not the pair.
    #[serde(default)]
    pub(crate) symbol: String,
    /// `BTC-USDT` on the linear endpoint; absent elsewhere.
    #[serde(default)]
    pub(crate) pair: String,
    /// The quote and settlement currency on the linear endpoint.
    #[serde(default)]
    pub(crate) trade_partition: String,
    /// The price tick, as a JSON number.
    #[serde(default)]
    pub(crate) price_tick: Num,
    /// Units of the underlying per contract.
    #[serde(default)]
    pub(crate) contract_size: Num,
    /// `1` while the contract is live.
    #[serde(default)]
    pub(crate) contract_status: i64,
    /// `swap`, `this_week`, `next_week`, `quarter`, …
    #[serde(default)]
    pub(crate) contract_type: String,
    /// Unix milliseconds as a string; empty on a perpetual.
    #[serde(default)]
    pub(crate) delivery_time: Num,
}
