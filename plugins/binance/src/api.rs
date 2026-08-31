//! The subset of Binance's `exchangeInfo` documents this crate reads.
//!
//! One shape covers all three markets — spot (`api`), USDⓈ-margined futures
//! (`fapi`) and coin-margined futures (`dapi`) — because they differ only in
//! which fields they populate, not in structure. Only consumed fields are
//! declared and every container defaults to empty, so a venue-side addition,
//! removal or rename of anything else cannot break decoding. Inbound only:
//! nothing here is ever serialised.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExchangeInfo {
    #[serde(default)]
    pub(crate) symbols: Vec<RawSymbol>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawSymbol {
    pub(crate) symbol: String,
    /// Spot and USDⓈ-M call this `status`; coin-M calls the same thing
    /// `contractStatus`. Read both, prefer whichever is populated.
    #[serde(default)]
    pub(crate) status: String,
    #[serde(default)]
    pub(crate) contract_status: String,
    #[serde(default)]
    pub(crate) base_asset: String,
    #[serde(default)]
    pub(crate) quote_asset: String,
    /// Futures only: what the contract is collateralised in. `USDT` on the
    /// linear market, the base coin on the inverse one.
    #[serde(default)]
    pub(crate) margin_asset: String,
    /// Futures only: `PERPETUAL`, `CURRENT_QUARTER`, `NEXT_QUARTER`, …
    #[serde(default)]
    pub(crate) contract_type: String,
    /// Futures only, Unix milliseconds. Perpetuals carry a far-future
    /// sentinel rather than an absent value, so it is never read for them.
    #[serde(default)]
    pub(crate) delivery_date: Option<i64>,
    /// Coin-M only: units of the quote currency one contract represents.
    #[serde(default)]
    pub(crate) contract_size: Option<Num>,
    #[serde(default)]
    pub(crate) filters: Vec<Filter>,
}

impl RawSymbol {
    /// The venue's trading state, from whichever field this market uses.
    pub(crate) fn state(&self) -> &str {
        if self.status.is_empty() {
            &self.contract_status
        } else {
            &self.status
        }
    }

    /// The minimum price increment, from `PRICE_FILTER`.
    pub(crate) fn tick(&self) -> Option<&Num> {
        self.filters.iter().find_map(|f| match f {
            Filter::Price { tick_size } => Some(tick_size),
            _ => None,
        })
    }

    /// The minimum quantity increment, from `LOT_SIZE`.
    pub(crate) fn step(&self) -> Option<&Num> {
        self.filters.iter().find_map(|f| match f {
            Filter::Lot { step_size } => Some(step_size),
            _ => None,
        })
    }
}

/// One entry of a symbol's `filters` array, discriminated by `filterType`.
#[derive(Debug, Deserialize)]
#[serde(tag = "filterType", rename_all_fields = "camelCase")]
pub(crate) enum Filter {
    #[serde(rename = "PRICE_FILTER")]
    Price { tick_size: Num },
    #[serde(rename = "LOT_SIZE")]
    Lot { step_size: Num },
    #[serde(other)]
    Other,
}
