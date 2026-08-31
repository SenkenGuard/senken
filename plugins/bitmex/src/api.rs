//! The subset of BitMEX's `GET /api/v1/instrument/active` this crate reads.
//!
//! BitMEX omits absent keys entirely rather than sending `null`, so every
//! field that is not universal is optional here — `expiry` is simply not
//! present on a perpetual. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawInstrument {
    pub(crate) symbol: String,
    /// The base asset. BitMEX calls it the underlying.
    #[serde(default)]
    pub(crate) underlying: String,
    #[serde(default)]
    pub(crate) quote_currency: String,
    /// Settlement currency in its smallest-unit spelling — `XBt`,
    /// `USDt` — which needs upper-casing to name the currency itself.
    #[serde(default)]
    pub(crate) settl_currency: String,
    #[serde(default)]
    pub(crate) state: String,
    /// `FFWCSX` perpetual, `FFCCSX` dated future, `IFXXXP` index.
    #[serde(default, rename = "typ")]
    pub(crate) instrument_type: String,
    #[serde(default)]
    pub(crate) tick_size: Num,
    /// Quantity step, in contracts.
    #[serde(default)]
    pub(crate) lot_size: Num,
    #[serde(default)]
    pub(crate) is_inverse: bool,
    /// A contract margined in neither leg of its pair.
    #[serde(default)]
    pub(crate) is_quanto: bool,
    /// ISO 8601. Absent — not null — on a perpetual.
    #[serde(default)]
    pub(crate) expiry: Option<String>,
    /// Units per contract; negative on inverse contracts.
    #[serde(default)]
    pub(crate) multiplier: Num,
}
