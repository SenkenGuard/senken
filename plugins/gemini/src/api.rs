//! The subset of Gemini's `GET /v1/symbols/details/all` this crate reads.
//!
//! **Beware the field names.** Gemini's `tick_size` is the *quantity* step
//! and `quote_increment` is the *price* tick — the opposite of what the
//! names suggest on most venues. Mapping `tick_size` to the price tick is
//! silently wrong, so the fields are documented here rather than trusted by
//! name at the call site. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct RawSymbol {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) base_currency: String,
    #[serde(default)]
    pub(crate) quote_currency: String,
    #[serde(default)]
    pub(crate) status: String,
    /// `spot` or `swap`. Every Gemini swap is linear, so `contract_type`
    /// adds nothing and is not read; neither is `contract_price_currency`,
    /// which disagrees with `quote_currency` on some perpetuals.
    #[serde(default)]
    pub(crate) product_type: String,
    /// The **price** tick, despite the name.
    #[serde(default)]
    pub(crate) quote_increment: Num,
    /// The **quantity** step, despite the name.
    #[serde(default)]
    pub(crate) tick_size: Num,
}
