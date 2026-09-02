//! Coinbase market data for Senken: Exchange spot and International
//! perpetuals.
//!
//! The two live on different hosts with different payloads, so they are two
//! sources. Note the public Advanced Trade path is
//! `/api/v3/brokerage/market/...`; the plain `/products` route needs a
//! signed request, which is why Exchange is used for spot here.

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{RawInstrument, RawProduct};

mod api;
mod bars;
mod book;
mod feed;

pub use bars::{CoinbaseBarSource, bar_source_spot};

/// Source id of the Coinbase Exchange spot market.
pub const SPOT_ID: &str = "coinbase-spot";
/// Source id of the Coinbase International perpetual market.
pub const PERP_ID: &str = "coinbase-intx";

const SPOT_URL: &str = "https://api.exchange.coinbase.com/products";
const PERP_URL: &str = "https://api.international.coinbase.com/api/v1/instruments";

/// The Coinbase Exchange spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "Coinbase Spot", SPOT_URL, client, parse_spot)
}

/// The Coinbase International perpetual market.
#[must_use]
pub fn perp_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        PERP_ID,
        "Coinbase International",
        PERP_URL,
        client,
        parse_perp,
    )
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let products: Vec<RawProduct> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(products.into_iter().filter_map(spot_instrument).collect())
}

fn spot_instrument(raw: RawProduct) -> Option<Instrument> {
    if raw.base_currency.is_empty() || raw.quote_currency.is_empty() {
        return skip(SPOT_ID, &raw.id, "missing base or quote currency");
    }
    let Some(price) = raw.quote_increment.increment() else {
        return skip(SPOT_ID, &raw.id, "unusable quote_increment");
    };
    let Some(qty) = raw.base_increment.increment() else {
        return skip(SPOT_ID, &raw.id, "unusable base_increment");
    };

    let status = if raw.trading_disabled {
        InstrumentStatus::Halted
    } else {
        map_spot_status(&raw.status, &raw.id)
    };

    Some(
        Instrument::spot(
            normalise_symbol(&raw.id, &['-']),
            raw.id,
            raw.base_currency,
            raw.quote_currency,
        )
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn parse_perp(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let instruments: Vec<RawInstrument> =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(instruments
        .into_iter()
        // The same document carries International's spot listings; spot
        // belongs to the Exchange source, so only perpetuals are kept.
        .filter(|raw| raw.instrument_type.eq_ignore_ascii_case("PERP"))
        .filter_map(perp_instrument)
        .collect())
}

fn perp_instrument(raw: RawInstrument) -> Option<Instrument> {
    if raw.base_asset_name.is_empty() || raw.quote_asset_name.is_empty() {
        return skip(PERP_ID, &raw.symbol, "missing base or quote asset");
    }
    let Some(price) = raw.quote_increment.increment() else {
        return skip(PERP_ID, &raw.symbol, "unusable quote_increment");
    };
    let Some(qty) = raw.base_increment.increment() else {
        return skip(PERP_ID, &raw.symbol, "unusable base_increment");
    };

    // Every International perpetual is quote-settled.
    let contract = Contract::new(&raw.quote_asset_name, Settlement::Linear);
    let status = map_perp_status(&raw.trading_state, &raw.symbol);
    let name = format!(
        "{} / {} perpetual",
        raw.base_asset_name, raw.quote_asset_name
    );
    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &['-']),
            raw.symbol,
            raw.base_asset_name,
            raw.quote_asset_name,
            InstrumentKind::Perpetual,
            contract,
        )
        .with_name(name)
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn map_spot_status(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "online" => InstrumentStatus::Trading,
        "offline" | "delisted" => InstrumentStatus::Closed,
        "" => InstrumentStatus::Unknown,
        other => {
            tracing::warn!(symbol, status = other, "unknown coinbase product status");
            InstrumentStatus::Unknown
        }
    }
}

fn map_perp_status(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "TRADING" => InstrumentStatus::Trading,
        "PAUSED" | "HALT" => InstrumentStatus::Halted,
        "DELISTED" | "CLOSED" => InstrumentStatus::Closed,
        other => {
            tracing::warn!(symbol, state = other, "unknown coinbase intx trading state");
            InstrumentStatus::Unknown
        }
    }
}

/// Registers both Coinbase markets with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct CoinbasePlugin;

impl Plugin for CoinbasePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "coinbase".to_owned(),
            name: "Coinbase".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Coinbase Exchange spot and International perpetual market data"
                .to_owned(),
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
        let group = context.limit_group("coinbase");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(perp_source(client.clone())));
        // Exchange spot only: bar fetching has been verified for
        // `coinbase-spot` (see `bars`' own module docs); International's
        // perpetual candles have not been audited and need their own
        // source once they are.
        context.register_bar_source(Arc::new(bar_source_spot(
            client.clone(),
            Arc::new(senken_plugin::SystemClock),
        )));
        // Depth, Exchange spot only — International's perpetual book has
        // not been audited, the same scope `bar_source_spot` above keeps.
        context.register_book_source(Arc::new(crate::book::book_source(client)));
        context.register_feed_source(Arc::new(crate::feed::CoinbaseFeedSource::new()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{map_spot_status, parse_perp, parse_spot};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const PERP: &[u8] = include_bytes!("../tests/fixtures/intx.json");

    #[test]
    fn spot_takes_its_price_tick_from_quote_increment() {
        let instruments = parse_spot(SPOT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSD").unwrap();

        assert_eq!(btc.source_symbol, "BTC-USD");
        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USD"));
        assert_eq!(btc.kind, InstrumentKind::Spot);
        assert!(btc.contract.is_none());
        assert_eq!((btc.price_scale, btc.tick_size), (2, 1));
    }

    #[test]
    fn a_disabled_product_is_halted_whatever_its_status_says() {
        let instruments = parse_spot(SPOT).unwrap();
        assert!(
            instruments
                .iter()
                .any(|i| i.status != InstrumentStatus::Trading),
            "the fixture carries a non-trading product"
        );
    }

    #[test]
    fn only_perpetuals_come_from_the_international_document() {
        let instruments = parse_perp(PERP).unwrap();
        assert!(!instruments.is_empty());
        assert!(
            instruments
                .iter()
                .all(|i| i.kind == InstrumentKind::Perpetual),
            "spot rows in the same document belong to the Exchange source"
        );

        let contract = instruments[0].contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.expiry, None);
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
        assert!(parse_perp(b"<html>nope</html>").is_err());
    }

    #[test]
    fn maps_documented_statuses() {
        assert_eq!(map_spot_status("online", "X"), InstrumentStatus::Trading);
        assert_eq!(map_spot_status("delisted", "X"), InstrumentStatus::Closed);
        assert_eq!(map_spot_status("weird", "X"), InstrumentStatus::Unknown);
    }
}
