//! WhiteBIT market data for Senken: spot pairs and perpetual futures.
//!
//! One endpoint returns both, so this is a single source; `type` is what
//! tells them apart. Every WhiteBIT perpetual is linear, settled in its
//! quote currency.

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::RawMarket;

mod api;

/// Source id of the WhiteBIT market.
pub const SOURCE_ID: &str = "whitebit";

const URL: &str = "https://whitebit.com/api/v4/public/markets";

/// Every WhiteBIT market: spot pairs and perpetuals.
#[must_use]
pub fn source(client: VenueClient) -> HttpSource {
    HttpSource::new(SOURCE_ID, "WhiteBIT", URL, client, parse)
}

fn parse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let markets: Vec<RawMarket> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(markets.into_iter().filter_map(to_instrument).collect())
}

fn to_instrument(raw: RawMarket) -> Option<Instrument> {
    if raw.stock.is_empty() || raw.money.is_empty() {
        return skip(SOURCE_ID, &raw.name, "missing stock or money leg");
    }
    let Some(price) = raw.tick_size.increment() else {
        return skip(SOURCE_ID, &raw.name, "unusable tickSize");
    };
    let Some(qty) = raw.step_size.increment() else {
        return skip(SOURCE_ID, &raw.name, "unusable stepSize");
    };

    let status = if raw.delisted_at.is_some() {
        InstrumentStatus::Closed
    } else if raw.trades_enabled {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Halted
    };
    let symbol = normalise_symbol(&raw.name, &['_']);

    let instrument = if raw.market_type == "futures" {
        let name = format!("{} / {} perpetual", raw.stock, raw.money);
        let contract = Contract::new(&raw.money, Settlement::Linear);
        Instrument::derivative(
            symbol,
            raw.name,
            raw.stock,
            raw.money,
            InstrumentKind::Perpetual,
            contract,
        )
        .with_name(name)
    } else {
        Instrument::spot(symbol, raw.name, raw.stock, raw.money)
    };

    Some(
        instrument
            .with_status(status)
            .with_price_increment(price)
            .with_qty_increment(qty),
    )
}

/// Registers the WhiteBIT market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct WhitebitPlugin;

impl Plugin for WhitebitPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "whitebit".to_owned(),
            name: "WhiteBIT".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "WhiteBIT spot and perpetual market data".to_owned(),
            permissions: Vec::new(),
        }
    }

    fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        let group = context.limit_group("whitebit");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(source(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/markets.json");

    #[test]
    fn spot_reads_its_legs_from_stock_and_money() {
        let instruments = parse(FIXTURE).unwrap();
        let spot = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Spot)
            .expect("the fixture carries a spot pair");

        assert!(!spot.base.is_empty() && !spot.quote.is_empty());
        assert!(spot.contract.is_none());
    }

    #[test]
    fn futures_are_linear_perpetuals_settled_in_the_quote() {
        let instruments = parse(FIXTURE).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Perpetual)
            .expect("the fixture carries a perpetual");

        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.settle, perp.quote);
        assert_eq!(contract.expiry, None);
    }

    #[test]
    fn a_delisted_market_is_closed() {
        let instruments = parse(FIXTURE).unwrap();
        assert!(
            instruments
                .iter()
                .any(|i| i.status == InstrumentStatus::Closed),
            "the fixture carries a delisted market"
        );
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse(b"<html>nope</html>").is_err());
    }
}
