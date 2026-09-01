//! Bitstamp market data for Senken: spot pairs and linear perpetuals.
//!
//! One endpoint returns both, told apart by `market_type`. Bitstamp's
//! perpetuals are not all crypto — gold, oil and index products are listed
//! alongside — and both increments arrive as decimal place counts rather
//! than sizes.

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::RawMarket;

mod api;

/// Source id of the Bitstamp market.
pub const SOURCE_ID: &str = "bitstamp";

const URL: &str = "https://www.bitstamp.net/api/v2/markets/";

/// Every Bitstamp market: spot pairs and perpetuals.
#[must_use]
pub fn source(client: VenueClient) -> HttpSource {
    HttpSource::new(SOURCE_ID, "Bitstamp", URL, client, parse)
}

fn parse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let markets: Vec<RawMarket> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(markets.into_iter().filter_map(to_instrument).collect())
}

fn to_instrument(raw: RawMarket) -> Option<Instrument> {
    if raw.base_currency.is_empty() || raw.counter_currency.is_empty() {
        return skip(SOURCE_ID, &raw.market_symbol, "missing base or counter leg");
    }
    // Bitstamp publishes only decimal-place counts, never an increment.
    let Some(price) = raw.counter_decimals.precision() else {
        return skip(SOURCE_ID, &raw.market_symbol, "unusable counter_decimals");
    };
    let Some(qty) = raw.base_decimals.precision() else {
        return skip(SOURCE_ID, &raw.market_symbol, "unusable base_decimals");
    };

    let status = if raw.trading.eq_ignore_ascii_case("enabled") {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Halted
    };
    let symbol = normalise_symbol(&raw.market_symbol, &['-']);

    let instrument = if raw.market_type.eq_ignore_ascii_case("PERPETUAL") {
        // Every Bitstamp perpetual has a linear payoff, settled in the
        // counter currency.
        let mut contract = Contract::new(&raw.counter_currency, Settlement::Linear);
        if let Some((scale, size)) = raw.contract_size.increment() {
            contract = contract.with_contract_size(scale, size);
        }
        let name = format!("{} / {} perpetual", raw.base_currency, raw.counter_currency);
        Instrument::derivative(
            symbol,
            raw.market_symbol,
            raw.base_currency,
            raw.counter_currency,
            InstrumentKind::Perpetual,
            contract,
        )
        .with_name(name)
    } else {
        let spot = Instrument::spot(
            symbol,
            raw.market_symbol,
            raw.base_currency,
            raw.counter_currency,
        );
        if raw.name.is_empty() {
            spot
        } else {
            spot.with_name(raw.name.replace('/', " / "))
        }
    };

    Some(
        instrument
            .with_status(status)
            .with_price_increment(price)
            .with_qty_increment(qty),
    )
}

/// Registers the Bitstamp market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct BitstampPlugin;

impl Plugin for BitstampPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "bitstamp".to_owned(),
            name: "Bitstamp".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Bitstamp spot and perpetual market data".to_owned(),
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
        let group = context.limit_group("bitstamp");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(source(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use senken_marketdata::instrument::{InstrumentKind, Settlement};

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/markets.json");

    #[test]
    fn decimal_place_counts_become_increments() {
        let instruments = parse(FIXTURE).unwrap();
        let spot = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Spot)
            .expect("the fixture carries a spot pair");

        // A count of N means a tick of 1 at scale N.
        assert_eq!(spot.tick_size, 1);
        assert_eq!(spot.step_size, 1);
        assert!(spot.contract.is_none());
    }

    #[test]
    fn the_quote_leg_is_the_counter_currency() {
        let instruments = parse(FIXTURE).unwrap();
        let btc = instruments
            .iter()
            .find(|i| i.source_symbol == "btcusd")
            .expect("the fixture carries btcusd");
        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USD"));
        assert_eq!(btc.symbol, "BTCUSD", "the venue id is lower case");
    }

    #[test]
    fn perpetuals_are_linear_and_keep_their_contract_size() {
        let instruments = parse(FIXTURE).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Perpetual)
            .expect("the fixture carries a perpetual");

        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.settle, perp.quote);
        assert_eq!(contract.expiry, None);
        assert!(contract.contract_size > 0);
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse(b"<html>nope</html>").is_err());
    }
}
