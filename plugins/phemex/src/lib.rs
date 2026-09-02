//! Phemex market data for Senken: spot pairs, inverse perpetuals and
//! linear perpetuals.
//!
//! All three arrive in one document, but they are **two** sources.
//! Phemex marks a spot symbol with a leading lower-case `s`, so `sOLUSDT`
//! is spot OL/USDT while `SOLUSDT` is the SOL/USDT perpetual. That marker
//! is a market, not part of the symbol: dropping it makes spot comparable
//! across venues, and keeping the two markets in separate sources is what
//! stops the pair from colliding once the case is normalised.
//!
//! Phemex quotes live prices as scaled integers elsewhere in its API, but
//! the product list reports plain increments — spot writes them with their
//! currency attached (`"0.01 USDT"`), contracts as a bare `tickSize`.

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{ProductsResponse, RawProduct};

mod api;
mod bars;
mod book;
mod feed;
pub mod scales;

pub use bars::{PhemexBarSource, bar_source_perp, bar_source_spot};
pub use book::{PhemexBookSource, book_source};
pub use scales::{ScaleCatalog, Scales};

/// Source id of the spot market.
pub const SPOT_ID: &str = "phemex-spot";
/// Source id of the perpetual market.
pub const PERP_ID: &str = "phemex-perp";

const URL: &str = crate::scales::PRODUCTS_URL;
/// The marker Phemex puts in front of every spot symbol.
const SPOT_PREFIX: char = 's';

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "Phemex Spot", URL, client, parse_spot)
}

/// The perpetual market, both generations: inverse and linear.
#[must_use]
pub fn perp_source(client: VenueClient) -> HttpSource {
    HttpSource::new(PERP_ID, "Phemex Perpetuals", URL, client, parse_perp)
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, true)
}

fn parse_perp(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, false)
}

fn parse(body: &[u8], want_spot: bool) -> Result<Vec<Instrument>, SourceError> {
    let response: ProductsResponse = serde_json::from_slice(body).map_err(SourceError::decode)?;
    if response.code != 0 {
        return Err(SourceError::rejected(format!(
            "code {}: {}",
            response.code, response.msg
        )));
    }
    let both = response
        .data
        .products
        .into_iter()
        .chain(response.data.perp_products_v2);
    Ok(both
        .filter(|raw| raw.product_type.eq_ignore_ascii_case("Spot") == want_spot)
        .filter_map(to_instrument)
        .collect())
}

fn to_instrument(raw: RawProduct) -> Option<Instrument> {
    let spot = raw.product_type.eq_ignore_ascii_case("Spot");
    let source = if spot { SPOT_ID } else { PERP_ID };
    if raw.quote_currency.is_empty() {
        return skip(source, &raw.symbol, "missing quote currency");
    }
    let Some(base) = base_of(&raw) else {
        return skip(source, &raw.symbol, "cannot resolve the base currency");
    };
    // Spot describes its increments as `"0.001 TRY"` under different keys;
    // only the contract arrays carry a plain `tickSize`.
    let Some(price) = (if spot {
        amount_increment(&raw.quote_tick_size)
    } else {
        raw.tick_size.increment()
    }) else {
        return skip(source, &raw.symbol, "unusable price increment");
    };
    let qty = if spot {
        amount_increment(&raw.base_tick_size)
    } else {
        // The V2 array publishes no lot size; those contracts step by one.
        raw.lot_size.increment()
    }
    .unwrap_or((0, 1));

    let status = if raw.status.eq_ignore_ascii_case("Listed") {
        InstrumentStatus::Trading
    } else if raw.status.eq_ignore_ascii_case("Delisted") {
        InstrumentStatus::Closed
    } else {
        InstrumentStatus::Halted
    };
    // The `s` marks the market, not the instrument: without dropping it
    // spot `sOLUSDT` upper-cases onto the `SOLUSDT` perpetual.
    let venue_symbol = if spot {
        raw.symbol.strip_prefix(SPOT_PREFIX).unwrap_or(&raw.symbol)
    } else {
        &raw.symbol
    };
    let symbol = normalise_symbol(venue_symbol, &[]);

    let instrument = if spot {
        Instrument::spot(symbol, raw.symbol, &base, &raw.quote_currency)
    } else {
        // A contract settling in its base is inverse; Phemex's original
        // perpetuals are all of that kind, the V2 ones are linear.
        let settle = if raw.settle_currency.is_empty() {
            raw.quote_currency.clone()
        } else {
            raw.settle_currency.clone()
        };
        let settlement = if settle.eq_ignore_ascii_case(&base) {
            Settlement::Inverse
        } else {
            Settlement::Linear
        };

        let name = format!("{base} / {} perpetual", raw.quote_currency);
        let mut contract = Contract::new(settle, settlement);
        if let Some((scale, size)) = raw.contract_size.increment() {
            contract = contract.with_contract_size(scale, size);
        }
        Instrument::derivative(
            symbol,
            raw.symbol,
            &base,
            raw.quote_currency,
            InstrumentKind::Perpetual,
            contract,
        )
        .with_name(name)
    };

    Some(
        instrument
            .with_status(status)
            .with_price_increment(price)
            .with_qty_increment(qty),
    )
}

/// Reads an increment that Phemex writes with its currency attached —
/// `"0.001 TRY"` — by taking the number off the front.
fn amount_increment(amount: &str) -> Option<(u8, i64)> {
    let number = amount.split_whitespace().next()?;
    senken_venue::Num::from(number).increment()
}

/// The base currency. The V2 array names it; the older one does not, so it
/// is recovered by stripping the quote from the symbol — `BTCUSD` quoted in
/// `USD` is `BTC`. Spot symbols carry a leading `s`, which is not part of
/// any currency code.
fn base_of(raw: &RawProduct) -> Option<String> {
    if !raw.base_currency.is_empty() {
        return Some(raw.base_currency.clone());
    }
    let symbol = raw.symbol.strip_prefix(SPOT_PREFIX).unwrap_or(&raw.symbol);
    let base = symbol.strip_suffix(&raw.quote_currency)?;
    (!base.is_empty()).then(|| base.to_owned())
}

/// Registers the Phemex market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct PhemexPlugin;

impl Plugin for PhemexPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "phemex".to_owned(),
            name: "Phemex".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Phemex spot and perpetual market data".to_owned(),
            permissions: Vec::new(),
        }
    }

    fn requires_http(&self) -> bool {
        true
    }

    fn activate_with_http(
        &self,
        context: &mut HttpActivationContext<'_>,
    ) -> Result<(), PluginError> {
        let group = context.limit_group("phemex");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(perp_source(client.clone())));

        // Every Phemex number is written at a scale the venue publishes
        // per symbol, so all three capabilities below share one catalogue
        // of those scales rather than each fetching the product list.
        let scales = crate::scales::ScaleCatalog::new(client.clone());
        context.register_bar_source(Arc::new(crate::bars::bar_source_spot(
            client.clone(),
            scales.clone(),
        )));
        context.register_bar_source(Arc::new(crate::bars::bar_source_perp(
            client.clone(),
            scales.clone(),
        )));
        context.register_book_source(Arc::new(crate::book::book_source(
            SPOT_ID,
            client.clone(),
            scales.clone(),
        )));
        context.register_book_source(Arc::new(crate::book::book_source(
            PERP_ID,
            client.clone(),
            scales.clone(),
        )));
        context.register_feed_source(Arc::new(crate::feed::PhemexFeedSource::new(
            SPOT_ID,
            client.clone(),
            scales.clone(),
        )));
        context.register_feed_source(Arc::new(crate::feed::PhemexFeedSource::new(
            PERP_ID,
            client.clone(),
            scales,
        )));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_perp, parse_spot};
    use senken_marketdata::instrument::{InstrumentKind, Settlement};
    use senken_marketdata::source::SourceError;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/products.json");

    #[test]
    fn an_inverse_perpetual_recovers_its_base_from_the_symbol() {
        // The original array names no base currency at all.
        let instruments = parse_perp(FIXTURE).unwrap();
        let btc = instruments
            .iter()
            .find(|i| i.source_symbol == "BTCUSD")
            .expect("the fixture carries BTCUSD");

        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USD"));
        assert_eq!(btc.kind, InstrumentKind::Perpetual);
        let contract = btc.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Inverse);
        assert_eq!(contract.settle, "BTC");
    }

    fn settlements(instruments: &[senken_marketdata::Instrument]) -> Vec<Settlement> {
        instruments
            .iter()
            .filter_map(|i| i.contract.as_ref().map(|c| c.settlement))
            .collect()
    }

    #[test]
    fn both_product_arrays_are_read_from_one_document() {
        let instruments = parse_perp(FIXTURE).unwrap();
        assert!(
            settlements(&instruments).contains(&Settlement::Linear),
            "the V2 array carries the linear perpetuals"
        );
        assert!(settlements(&instruments).contains(&Settlement::Inverse));
    }

    #[test]
    fn the_spot_marker_never_becomes_part_of_the_symbol() {
        // `sOLUSDT` is spot OL/USDT; `SOLUSDT` is the SOL perpetual. Upper
        // casing the marker collapses one onto the other.
        let spot = parse_spot(FIXTURE).unwrap();
        assert!(
            spot.iter().all(|i| !i.symbol.starts_with('S')
                || !i.source_symbol.starts_with('s')
                || i.symbol == i.source_symbol[1..].to_uppercase()),
            "the leading `s` marks the market, not the symbol"
        );
        assert!(
            spot.iter().any(|i| i.source_symbol.starts_with('s')),
            "the fixture carries a marked spot symbol"
        );
    }

    #[test]
    fn each_market_is_its_own_source() {
        // One document, two catalogs: keeping them together would let a
        // spot symbol and a perpetual share one normalised name.
        assert!(
            parse_spot(FIXTURE)
                .unwrap()
                .iter()
                .all(|i| i.contract.is_none())
        );
        assert!(
            parse_perp(FIXTURE)
                .unwrap()
                .iter()
                .all(|i| i.contract.is_some())
        );
    }

    #[test]
    fn spot_reads_the_increments_written_with_their_currency() {
        // Phemex spot carries no `tickSize`; it writes `"0.01 USDT"` under
        // `quoteTickSize` instead. Reading only `tickSize` dropped every
        // spot pair the venue lists.
        let instruments = parse_spot(FIXTURE).unwrap();
        // Named rather than "the first spot pair": the fixture now holds
        // several, deliberately including two that quote in TRY, and each
        // writes a different increment.
        let spot = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Spot && i.symbol == "BTCUSDT")
            .expect("the fixture carries BTC/USDT spot");

        assert!(spot.contract.is_none());
        assert_eq!((spot.price_scale, spot.tick_size), (2, 1));
        assert_eq!((spot.qty_scale, spot.step_size), (6, 1));
    }

    #[test]
    fn a_v2_contract_steps_by_one_since_it_has_no_lot_size() {
        let instruments = parse_perp(FIXTURE).unwrap();
        let linear = instruments
            .iter()
            .find(|i| {
                i.contract
                    .as_ref()
                    .is_some_and(|c| c.settlement == Settlement::Linear)
            })
            .unwrap();
        assert_eq!((linear.qty_scale, linear.step_size), (0, 1));
    }

    #[test]
    fn a_failure_code_is_a_rejection() {
        let body = br#"{"code":401,"msg":"denied","data":{}}"#;
        assert!(matches!(
            parse_perp(body),
            Err(SourceError::Rejected { .. })
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
    }
}
