//! The subset of OKX's `GET /api/v5/public/instruments` this crate reads.
//!
//! One shape covers every `instType`. Note that OKX leaves `baseCcy` and
//! `quoteCcy` **empty on derivatives** and carries the pair in `uly`
//! instead, so those fields are optional here rather than required. Only
//! consumed fields are declared and every container defaults to empty, so a
//! venue-side change elsewhere cannot break decoding. Inbound only: nothing
//! here is ever serialised.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct InstrumentsResponse {
    pub(crate) code: String,
    #[serde(default)]
    pub(crate) msg: String,
    #[serde(default)]
    pub(crate) data: Vec<RawInstrument>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawInstrument {
    pub(crate) inst_id: String,
    /// Populated on spot; empty on every derivative.
    #[serde(default)]
    pub(crate) base_ccy: String,
    #[serde(default)]
    pub(crate) quote_ccy: String,
    /// The underlying pair, `BTC-USD`. Where a derivative's base and quote
    /// are read from, since `baseCcy`/`quoteCcy` are empty there.
    #[serde(default)]
    pub(crate) uly: String,
    /// The instrument family, `BTC-USD`. Some index-tracking swaps fill
    /// this in and leave `uly` empty.
    #[serde(default)]
    pub(crate) inst_family: String,
    /// What the contract settles in.
    #[serde(default)]
    pub(crate) settle_ccy: String,
    /// `linear` or `inverse`; empty on spot.
    #[serde(default)]
    pub(crate) ct_type: String,
    /// Units of `ctValCcy` per contract.
    #[serde(default)]
    pub(crate) ct_val: Num,
    /// Expiry in Unix milliseconds; empty on spot and perpetual swaps.
    #[serde(default)]
    pub(crate) exp_time: Num,
    /// `C` or `P` on options; empty otherwise.
    #[serde(default)]
    pub(crate) opt_type: String,
    /// Option strike.
    #[serde(default)]
    pub(crate) stk: Num,
    pub(crate) tick_sz: Num,
    pub(crate) lot_sz: Num,
    pub(crate) state: String,
}
