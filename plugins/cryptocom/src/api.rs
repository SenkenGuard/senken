//! The subset of Crypto.com's `GET /exchange/v1/public/get-instruments`
//! this crate reads.
//!
//! One document covers spot, perpetual swaps and dated futures, told apart
//! by `inst_type`. Note the v1 envelope reports success as the **integer**
//! `0`, where the retired v2 API used a string. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct InstrumentsResponse {
    #[serde(default)]
    pub(crate) code: i64,
    #[serde(default)]
    pub(crate) message: String,
    #[serde(default)]
    pub(crate) result: InstrumentsResult,
}

#[derive(Debug, Default, Deserialize)]
pub(crate) struct InstrumentsResult {
    #[serde(default)]
    pub(crate) data: Vec<RawInstrument>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawInstrument {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_ccy: String,
    #[serde(default)]
    pub(crate) quote_ccy: String,
    /// `CCY_PAIR`, `PERPETUAL_SWAP` or `FUTURE`.
    #[serde(default)]
    pub(crate) inst_type: String,
    #[serde(default)]
    pub(crate) tradable: bool,
    #[serde(default)]
    pub(crate) price_tick_size: Num,
    #[serde(default)]
    pub(crate) qty_tick_size: Num,
    #[serde(default)]
    pub(crate) contract_size: Num,
    /// Unix milliseconds; `0` on spot and perpetuals.
    #[serde(default)]
    pub(crate) expiry_timestamp_ms: Num,
}
