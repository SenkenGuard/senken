//! Phemex's per-symbol number scales, fetched from its own product list.
//!
//! # Why this module has to exist
//!
//! Every other venue in this workspace writes a price as decimal text and
//! the reader counts its digits. Phemex writes some of its prices as
//! **integers pre-multiplied by a per-symbol power of ten**, and publishes
//! the exponent as `priceScale` on each product. Confirmed live
//! 2026-09-02 against `GET /public/products` — 1177 products, every one of
//! them carrying the field:
//!
//! | family | `priceScale` | what a price looks like |
//! |---|---|---|
//! | `perpProductsV2` (883 linear perps) | `0` | `"79149.9"` |
//! | spot (1012 symbols) | `8` | `"7917979000000"` |
//! | spot (**6** symbols) | `4` | `"..."` |
//! | inverse perpetuals (159) | `4` | `"791301000"` |
//!
//! So `priceScale` is not merely a divisor: **a scale of zero is the
//! venue saying "this one is ordinary decimal text"**, and the same
//! endpoint answers in either shape depending on the symbol asked for.
//!
//! The six spot symbols at scale 4 sitting among 1012 at scale 8 are the
//! whole argument for looking this up per symbol. `sBTCTRY` read at spot's
//! "usual" scale of 8 is wrong by a factor of ten thousand, and wrong in
//! the direction that still looks like a plausible price.
//!
//! This is also why nothing in this plugin was registered before: an
//! earlier reading of the product list concluded the field was absent,
//! because the recorded fixture it was checked against had been trimmed
//! field by field and `priceScale` was one of the fields dropped.

use std::collections::HashMap;
use std::sync::Arc;

use senken_marketdata::source::SourceError;
use senken_venue::VenueClient;
use tokio::sync::OnceCell;

use crate::api::ProductsResponse;

/// `GET /public/products` — the same document the instrument catalog is
/// built from.
pub const PRODUCTS_URL: &str = "https://api.phemex.com/public/products";

/// Weight charged for the one product-list fetch this catalogue makes.
const PRODUCTS_FETCH_COST: u32 = 1;

/// How one symbol's numbers are written on the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scales {
    /// Power of ten a price field is pre-multiplied by. `0` means the
    /// field is ordinary decimal text.
    pub price: u8,
    /// Power of ten a quantity field is pre-multiplied by, derived from
    /// the venue's own `baseTickSize` and its `Ev` twin. `0` means either
    /// decimal text or a plain contract count.
    pub quantity: u8,
    /// Power of ten a turnover field is pre-multiplied by — the venue's
    /// own `ratioScale`.
    pub ratio: u8,
}

impl Scales {
    /// Whether this symbol's numbers arrive as decimal text rather than
    /// pre-scaled integers.
    #[must_use]
    pub const fn is_decimal(self) -> bool {
        self.price == 0
    }
}

/// Every symbol's scales, fetched once and shared.
///
/// Fetched lazily rather than at activation: a plugin that registers four
/// capabilities should not make the runtime wait on a 2.5 MB document
/// before the server can listen, and a source that is never used should
/// not fetch it at all.
#[derive(Debug, Clone)]
pub struct ScaleCatalog {
    client: VenueClient,
    url: String,
    loaded: Arc<OnceCell<HashMap<String, Scales>>>,
}

impl ScaleCatalog {
    /// A catalogue that fetches Phemex's product list through `client`.
    #[must_use]
    pub fn new(client: VenueClient) -> Self {
        Self {
            client,
            url: PRODUCTS_URL.to_owned(),
            loaded: Arc::new(OnceCell::new()),
        }
    }

    /// Points the catalogue at a different URL — a local stand-in in
    /// tests, exactly as this plugin's other sources take one.
    #[must_use]
    pub fn with_url(mut self, url: impl Into<String>) -> Self {
        self.url = url.into();
        self
    }

    /// Loads the product list if it is not loaded already.
    ///
    /// Exists for the live feed, whose frame decoding is synchronous:
    /// warming here — from the one place in a connection's life that *is*
    /// async, the dial — means [`cached`](Self::cached) has an answer by
    /// the time frames arrive.
    ///
    /// # Errors
    /// As [`get`](Self::get).
    pub(crate) async fn warm(&self) -> Result<(), SourceError> {
        self.get_or_load().await.map(|_| ())
    }

    /// The scales for `symbol` if the product list is already loaded.
    ///
    /// Never fetches. A caller that has not warmed the catalogue gets
    /// `None` and must drop the frame rather than read it at a guessed
    /// scale.
    pub(crate) fn cached(&self, symbol: &str) -> Option<Scales> {
        self.loaded.get()?.get(symbol).copied()
    }

    /// The scales for `symbol`.
    ///
    /// # Errors
    /// [`SourceError`] if the product list could not be fetched or
    /// decoded, or if it does not describe `symbol` — refusing is the
    /// point. A default scale guessed for an unknown symbol is how a
    /// price ends up wrong by four orders of magnitude with nothing to
    /// show for it.
    pub(crate) async fn get(&self, symbol: &str) -> Result<Scales, SourceError> {
        let all = self.get_or_load().await?;
        all.get(symbol).copied().ok_or_else(|| {
            SourceError::rejected(format!(
                "Phemex's product list does not describe {symbol}, so its number scale is unknown"
            ))
        })
    }

    async fn get_or_load(&self) -> Result<&HashMap<String, Scales>, SourceError> {
        self.loaded
            .get_or_try_init(|| async {
                let body = self.client.get(&self.url, PRODUCTS_FETCH_COST).await?;
                let parsed: ProductsResponse =
                    serde_json::from_slice(&body).map_err(SourceError::decode)?;
                if parsed.code != 0 {
                    return Err(SourceError::rejected(format!(
                        "code {}: {}",
                        parsed.code, parsed.msg
                    )));
                }
                Ok(scales_of(&parsed))
            })
            .await
    }
}

/// Builds the symbol → scales map from a decoded product list.
fn scales_of(parsed: &ProductsResponse) -> HashMap<String, Scales> {
    parsed
        .data
        .products
        .iter()
        .chain(parsed.data.perp_products_v2.iter())
        .map(|product| {
            (
                product.symbol.clone(),
                Scales {
                    price: product.price_scale,
                    quantity: quantity_scale(product),
                    ratio: product.ratio_scale,
                },
            )
        })
        .collect()
}

/// The power of ten a quantity is pre-multiplied by.
///
/// Derived rather than read: Phemex publishes no `qtyScale`, but it does
/// publish the same tick twice — `baseTickSize: "0.000001 BTC"` beside
/// `baseTickSizeEv: 100` — and the ratio between them *is* the scale
/// (`100 / 0.000001 = 10^8`). Deriving it from the venue's own pair is
/// exact; assuming it matches `priceScale` would be a guess that happens
/// to hold for `sBTCUSDT` and has no reason to hold anywhere else.
///
/// Returns `0` where there is no such pair — an inverse perpetual's
/// quantity is a plain contract count, and a V2 linear one is decimal
/// text.
fn quantity_scale(product: &crate::api::RawProduct) -> u8 {
    let Some(ev) = product.base_tick_size_ev else {
        return 0;
    };
    let Some(tick) = product
        .base_tick_size
        .split_whitespace()
        .next()
        .and_then(senken_core::plain_decimal)
    else {
        return 0;
    };
    let digits = senken_core::decimal_places(&tick);
    let Some(unit) = senken_core::parse_scaled(&tick, digits) else {
        return 0;
    };
    if unit <= 0 || ev <= 0 {
        return 0;
    }
    // The tick is `unit × 10^-digits`, and its `Ev` twin is that same tick
    // multiplied by `10^scale`. So `ev / unit` is `10^(scale - digits)`,
    // and the tick's own fractional digits make up the difference.
    let mut ratio = ev / unit;
    if ev % unit != 0 {
        return 0;
    }
    let mut exponent = 0u8;
    while ratio > 1 && ratio % 10 == 0 {
        ratio /= 10;
        exponent = exponent.saturating_add(1);
    }
    if ratio != 1 {
        return 0;
    }
    exponent.saturating_add(digits)
}

#[cfg(test)]
mod tests {
    use super::{ScaleCatalog, Scales};
    use senken_venue::{LimitGroup, VenueClient};
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A real `GET /public/products` response, recorded 2026-09-02. Whole
    /// product objects, exactly as the venue wrote them — only *which*
    /// symbols are included is narrowed, because the full document is 1177
    /// products and 2.5 MB. The five kept cover every scale this venue
    /// uses.
    const PRODUCTS: &[u8] = include_bytes!("../tests/fixtures/products.json");

    async fn catalog() -> (MockServer, ScaleCatalog) {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(PRODUCTS, "application/json"))
            .mount(&server)
            .await;
        let client = VenueClient::new(reqwest::Client::new(), LimitGroup::new("phemex-test"));
        let catalog = ScaleCatalog::new(client).with_url(server.uri());
        (server, catalog)
    }

    /// The three families write their numbers three different ways, and
    /// the product list is the only thing that says which is which.
    #[tokio::test]
    async fn each_family_gets_the_scale_the_venue_published_for_it() {
        let (_server, catalog) = catalog().await;

        assert_eq!(
            catalog.get("BTCUSD").await.unwrap().price,
            4,
            "inverse perpetual: prices arrive as integers at 10^4"
        );
        assert_eq!(
            catalog.get("sBTCUSDT").await.unwrap().price,
            8,
            "spot: integers at 10^8"
        );
        assert_eq!(
            catalog.get("BTCUSDT").await.unwrap().price,
            0,
            "V2 linear perpetual: ordinary decimal text"
        );
    }

    /// The whole reason this is a per-symbol lookup. Six spot symbols use
    /// scale 4 while 1012 use scale 8; reading `sUSDTTRY` at spot's
    /// "usual" scale is wrong by ten thousand, and still looks plausible.
    #[tokio::test]
    async fn two_spot_symbols_on_the_same_venue_have_different_scales() {
        let (_server, catalog) = catalog().await;

        let common = catalog.get("sBTCUSDT").await.unwrap();
        let unusual = catalog.get("sUSDTTRY").await.unwrap();

        assert_eq!(common.price, 8);
        assert_eq!(unusual.price, 4);
        assert_ne!(
            common.price, unusual.price,
            "a single spot-wide constant would misprice one of these two"
        );
    }

    /// A scale of zero is the venue saying "decimal text", not "no
    /// scaling information".
    #[tokio::test]
    async fn a_zero_scale_reads_as_decimal_text() {
        let (_server, catalog) = catalog().await;
        assert!(catalog.get("BTCUSDT").await.unwrap().is_decimal());
        assert!(!catalog.get("BTCUSD").await.unwrap().is_decimal());
    }

    /// Derived from `baseTickSize` and `baseTickSizeEv`, which the venue
    /// publishes as a matched pair: `100 / 0.000001` is `10^8`.
    #[tokio::test]
    async fn a_spot_quantity_scale_is_derived_from_the_venues_own_tick_pair() {
        let (_server, catalog) = catalog().await;
        assert_eq!(catalog.get("sBTCUSDT").await.unwrap().quantity, 8);
    }

    /// An inverse perpetual's size is a contract count with no tick pair
    /// to derive from, and a V2 linear one is decimal text.
    #[tokio::test]
    async fn a_symbol_with_no_tick_pair_has_no_quantity_scale() {
        let (_server, catalog) = catalog().await;
        assert_eq!(catalog.get("BTCUSD").await.unwrap().quantity, 0);
        assert_eq!(catalog.get("BTCUSDT").await.unwrap().quantity, 0);
    }

    /// Refusing is the point: a default scale guessed for a symbol the
    /// venue did not describe is how a price ends up four orders of
    /// magnitude out with nothing to show for it.
    #[tokio::test]
    async fn an_unknown_symbol_is_refused_rather_than_given_a_default() {
        let (_server, catalog) = catalog().await;
        assert!(catalog.get("sNOTLISTED").await.is_err());
    }

    /// One fetch, however many symbols are asked about — the document is
    /// 2.5 MB and every source in this plugin consults it.
    #[tokio::test]
    async fn the_product_list_is_fetched_once_and_shared() {
        let (server, catalog) = catalog().await;
        for symbol in ["BTCUSD", "sBTCUSDT", "BTCUSDT", "BTCUSD"] {
            catalog.get(symbol).await.unwrap();
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    /// An inverse perpetual reports its traded size two ways and neither
    /// is a plain base amount: `volume` counts $1 contracts and
    /// `turnover` is the base asset at `ratioScale`.
    #[tokio::test]
    async fn an_inverse_perpetual_carries_a_turnover_scale() {
        let (_server, catalog) = catalog().await;
        assert_eq!(catalog.get("BTCUSD").await.unwrap().ratio, 8);
    }

    #[test]
    fn scales_are_comparable() {
        let one = Scales {
            price: 4,
            quantity: 0,
            ratio: 8,
        };
        assert_eq!(one, one);
    }
}
