//! The subset of Phemex's `GET /public/products` this crate reads.
//!
//! One document carries two arrays: `products` holds spot pairs and the
//! older inverse perpetuals, `perpProductsV2` the linear ones. Both are
//! read from the same fetch. Inbound only.

use senken_venue::Num;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ProductsResponse {
    #[serde(default)]
    pub(crate) code: i64,
    #[serde(default)]
    pub(crate) msg: String,
    #[serde(default)]
    pub(crate) data: ProductsData,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ProductsData {
    /// Spot pairs and the original, inverse perpetuals.
    #[serde(default)]
    pub(crate) products: Vec<RawProduct>,
    /// Linear perpetuals.
    #[serde(default)]
    pub(crate) perp_products_v2: Vec<RawProduct>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RawProduct {
    pub(crate) symbol: String,
    /// `Perpetual`, `PerpetualV2` or `Spot`.
    #[serde(default, rename = "type")]
    pub(crate) product_type: String,
    /// Present on the V2 array; absent on the older one, where the base has
    /// to be recovered from the symbol.
    #[serde(default)]
    pub(crate) base_currency: String,
    #[serde(default)]
    pub(crate) quote_currency: String,
    #[serde(default)]
    pub(crate) settle_currency: String,
    /// A JSON number on the older array, a decimal string on the V2 one.
    /// **Absent on spot**, which describes its increments differently.
    #[serde(default)]
    pub(crate) tick_size: Num,
    /// Spot only: the price increment, written with its currency —
    /// `"0.001 TRY"` — so the number has to be taken off the front.
    #[serde(default)]
    pub(crate) quote_tick_size: String,
    /// Spot only: the quantity increment, in the same form.
    #[serde(default)]
    pub(crate) base_tick_size: String,
    /// Quantity step, in contracts. Absent on the V2 array.
    #[serde(default)]
    pub(crate) lot_size: Num,
    #[serde(default)]
    pub(crate) contract_size: Num,
    /// `Listed` while the product trades.
    #[serde(default)]
    pub(crate) status: String,
}
