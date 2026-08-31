//! The subset of Bybit's `GET /v5/market/instruments-info` this crate reads.
//!
//! One shape covers every `category`. Only consumed fields are declared and
//! every container defaults to empty, so a venue-side change elsewhere
//! cannot break decoding. Inbound only: nothing here is ever serialised.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstrumentsResponse {
    pub(crate) ret_code: i64,
    #[serde(default)]
    pub(crate) ret_msg: String,
    #[serde(default)]
    pub(crate) result: InstrumentsResult,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InstrumentsResult {
    #[serde(default)]
    pub(crate) list: Vec<RawInstrument>,
    /// Non-empty when Bybit has more rows than one page holds.
    #[serde(default)]
    pub(crate) next_page_cursor: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawInstrument {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_coin: String,
    #[serde(default)]
    pub(crate) quote_coin: String,
    pub(crate) status: String,
    /// `LinearPerpetual`, `InverseFutures`, …; absent on spot and options.
    #[serde(default)]
    pub(crate) contract_type: String,
    #[serde(default)]
    pub(crate) settle_coin: String,
    /// `Call` or `Put`, on the options category only.
    #[serde(default)]
    pub(crate) options_type: String,
    /// Unix milliseconds; `"0"` on a perpetual.
    #[serde(default)]
    pub(crate) delivery_time: Num,
    #[serde(default)]
    pub(crate) price_filter: PriceFilter,
    #[serde(default)]
    pub(crate) lot_size_filter: LotSizeFilter,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PriceFilter {
    #[serde(default)]
    pub(crate) tick_size: Num,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LotSizeFilter {
    /// Spot names the quantity step `basePrecision`; every derivative
    /// category names the same thing `qtyStep`.
    #[serde(default)]
    pub(crate) base_precision: Num,
    #[serde(default)]
    pub(crate) qty_step: Num,
}

impl LotSizeFilter {
    pub(crate) fn step(&self) -> &Num {
        if self.qty_step.is_empty() {
            &self.base_precision
        } else {
            &self.qty_step
        }
    }
}
