//! The subset of Deribit's `GET /api/v2/public/get_instruments` this crate
//! reads.
//!
//! One document covers every kind — spot, futures, perpetuals and options —
//! under a JSON-RPC envelope. Deribit sends many numbers in scientific
//! notation (`6.9e4` for a strike), which [`Num`] normalises. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct InstrumentsResponse {
    #[serde(default)]
    pub(crate) result: Vec<RawInstrument>,
    #[serde(default)]
    pub(crate) error: Option<RpcError>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RpcError {
    #[serde(default)]
    pub(crate) code: i64,
    #[serde(default)]
    pub(crate) message: String,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawInstrument {
    pub(crate) instrument_name: String,
    #[serde(default)]
    pub(crate) base_currency: String,
    /// On an option this is the *premium* currency — `BTC` for an inverse
    /// BTC option — not the other leg of the pair. Prefer
    /// `counter_currency`, which names the leg on every kind.
    #[serde(default)]
    pub(crate) quote_currency: String,
    /// What the underlying is priced against: `USD` for every BTC contract,
    /// option or not.
    #[serde(default)]
    pub(crate) counter_currency: String,
    #[serde(default)]
    pub(crate) settlement_currency: String,
    /// `future`, `option`, `spot`, `future_combo`, `option_combo`.
    #[serde(default)]
    pub(crate) kind: String,
    /// `linear` or `reversed` (Deribit's word for inverse).
    #[serde(default)]
    pub(crate) instrument_type: String,
    /// `perpetual`, `day`, `week`, `month` — the only reliable way to tell
    /// a perpetual apart, since its expiry is a year-3000 sentinel.
    #[serde(default)]
    pub(crate) settlement_period: String,
    #[serde(default)]
    pub(crate) is_active: bool,
    #[serde(default)]
    pub(crate) tick_size: Num,
    #[serde(default)]
    pub(crate) min_trade_amount: Num,
    #[serde(default)]
    pub(crate) contract_size: Num,
    #[serde(default)]
    pub(crate) expiration_timestamp: Num,
    /// `call` or `put`, on options only.
    #[serde(default)]
    pub(crate) option_type: String,
    #[serde(default)]
    pub(crate) strike: Num,
}
