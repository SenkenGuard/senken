//! Gemini market data for Senken: spot pairs and linear perpetuals.
//!
//! One endpoint returns both, told apart by `product_type`. The venue lists
//! no dated futures and no options.

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::RawSymbol;

mod api;

/// Source id of the Gemini market.
pub const SOURCE_ID: &str = "gemini";

const URL: &str = "https://api.gemini.com/v1/symbols/details/all";

/// Every Gemini instrument: spot pairs and perpetuals.
#[must_use]
pub fn source(client: VenueClient) -> HttpSource {
    HttpSource::new(SOURCE_ID, "Gemini", URL, client, parse)
}

fn parse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let symbols: Vec<RawSymbol> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(symbols.into_iter().filter_map(to_instrument).collect())
}

fn to_instrument(raw: RawSymbol) -> Option<Instrument> {
    if raw.base_currency.is_empty() || raw.quote_currency.is_empty() {
        return skip(SOURCE_ID, &raw.symbol, "missing base or quote currency");
    }
    // `quote_increment` is the price tick and `tick_size` the quantity
    // step — Gemini's names are the reverse of the usual convention.
    let Some(price) = raw.quote_increment.increment() else {
        return skip(SOURCE_ID, &raw.symbol, "unusable quote_increment");
    };
    let Some(qty) = raw.tick_size.increment() else {
        return skip(SOURCE_ID, &raw.symbol, "unusable tick_size");
    };

    let symbol = normalise_symbol(&raw.symbol, &[]);
    let status = map_status(&raw.status, &raw.symbol);
    let instrument = if raw.product_type.eq_ignore_ascii_case("swap") {
        let contract = Contract::new(&raw.quote_currency, Settlement::Linear);
        let name = format!("{} / {} perpetual", raw.base_currency, raw.quote_currency);
        // Every Gemini perpetual is linear, so it settles in its quote
        // currency. `contract_price_currency` sometimes disagrees with
        // `quote_currency` — BTCUSDCPERP says GUSD — and the quote wins.
        Instrument::derivative(
            symbol,
            raw.symbol,
            raw.base_currency,
            raw.quote_currency,
            InstrumentKind::Perpetual,
            contract,
        )
        .with_name(name)
    } else {
        Instrument::spot(symbol, raw.symbol, raw.base_currency, &raw.quote_currency)
    };

    Some(
        instrument
            .with_status(status)
            .with_price_increment(price)
            .with_qty_increment(qty),
    )
}

fn map_status(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "open" => InstrumentStatus::Trading,
        "limit_only" | "cancel_only" => InstrumentStatus::Halted,
        "closed" => InstrumentStatus::Closed,
        other => {
            tracing::warn!(symbol, status = other, "unknown gemini symbol status");
            InstrumentStatus::Unknown
        }
    }
}

/// Registers the Gemini market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct GeminiPlugin;

impl Plugin for GeminiPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "gemini".to_owned(),
            name: "Gemini".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Gemini spot and perpetual market data".to_owned(),
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
        let group = context.limit_group("gemini");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(source(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{map_status, parse};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/symbols.json");

    #[test]
    fn the_price_tick_comes_from_quote_increment_not_tick_size() {
        // Gemini's `tick_size` is the quantity step. Reading it as the
        // price tick would make BTCUSD look like it ticks in satoshis.
        let instruments = parse(FIXTURE).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSD").unwrap();

        assert_eq!(
            (btc.price_scale, btc.tick_size),
            (2, 1),
            "BTCUSD ticks in cents"
        );
        assert_eq!(
            (btc.qty_scale, btc.step_size),
            (8, 1),
            "quantities step in satoshis"
        );
    }

    #[test]
    fn perpetuals_are_linear_and_never_expire() {
        let instruments = parse(FIXTURE).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Perpetual)
            .expect("the fixture carries a perpetual");

        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.expiry, None);
        assert_eq!(
            contract.settle, perp.quote,
            "quote_currency wins over contract_price_currency"
        );
    }

    #[test]
    fn spot_carries_no_contract() {
        let instruments = parse(FIXTURE).unwrap();
        let spot = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Spot)
            .unwrap();
        assert!(spot.contract.is_none());
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse(b"<html>nope</html>").is_err());
    }

    #[test]
    fn maps_documented_statuses() {
        assert_eq!(map_status("open", "X"), InstrumentStatus::Trading);
        assert_eq!(map_status("limit_only", "X"), InstrumentStatus::Halted);
        assert_eq!(map_status("closed", "X"), InstrumentStatus::Closed);
        assert_eq!(map_status("weird", "X"), InstrumentStatus::Unknown);
    }
}
