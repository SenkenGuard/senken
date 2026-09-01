//! Poloniex market data for Senken: spot pairs and perpetual futures.
//!
//! The two are separate endpoints with unrelated shapes, so each is its own
//! source. Spot reports decimal place counts; perpetuals report a real tick
//! size under abbreviated keys.

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{InstrumentsResponse, RawContract, RawSymbol};

mod api;

/// Source id of the spot market.
pub const SPOT_ID: &str = "poloniex-spot";
/// Source id of the perpetual market.
pub const PERP_ID: &str = "poloniex-perp";

const SPOT_URL: &str = "https://api.poloniex.com/markets";
const PERP_URL: &str = "https://api.poloniex.com/v3/market/allInstruments";

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "Poloniex Spot", SPOT_URL, client, parse_spot)
}

/// The perpetual market.
#[must_use]
pub fn perp_source(client: VenueClient) -> HttpSource {
    HttpSource::new(PERP_ID, "Poloniex Perpetuals", PERP_URL, client, parse_perp)
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let symbols: Vec<RawSymbol> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(symbols.into_iter().filter_map(spot_instrument).collect())
}

fn spot_instrument(raw: RawSymbol) -> Option<Instrument> {
    if raw.base_currency_name.is_empty() || raw.quote_currency_name.is_empty() {
        return skip(SPOT_ID, &raw.symbol, "missing base or quote currency");
    }
    // Poloniex spot publishes only decimal-place counts.
    let Some(price) = raw.symbol_trade_limit.price_scale.precision() else {
        return skip(SPOT_ID, &raw.symbol, "unusable priceScale");
    };
    let Some(qty) = raw.symbol_trade_limit.quantity_scale.precision() else {
        return skip(SPOT_ID, &raw.symbol, "unusable quantityScale");
    };

    let status = map_spot_state(&raw.state, &raw.symbol);
    Some(
        Instrument::spot(
            normalise_symbol(&raw.symbol, &['_']),
            raw.symbol,
            raw.base_currency_name,
            raw.quote_currency_name,
        )
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn parse_perp(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let response: InstrumentsResponse =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    if response.code != 0 && response.code != 200 {
        return Err(SourceError::rejected(format!(
            "code {}: {}",
            response.code, response.msg
        )));
    }
    Ok(response
        .data
        .into_iter()
        .filter_map(perp_instrument)
        .collect())
}

fn perp_instrument(raw: RawContract) -> Option<Instrument> {
    if raw.base_ccy.is_empty() || raw.quote_ccy.is_empty() {
        return skip(PERP_ID, &raw.symbol, "missing base or quote currency");
    }
    let Some(price) = raw.tick_size.increment() else {
        return skip(PERP_ID, &raw.symbol, "unusable tSz");
    };
    let Some(qty) = raw.lot_size.increment() else {
        return skip(PERP_ID, &raw.symbol, "unusable lotSz");
    };

    let settlement = if raw.contract_type.eq_ignore_ascii_case("INVERSE") {
        Settlement::Inverse
    } else {
        Settlement::Linear
    };
    let settle = if raw.settle_ccy.is_empty() {
        raw.quote_ccy.as_str()
    } else {
        raw.settle_ccy.as_str()
    };

    let name = format!("{} / {} perpetual", raw.base_ccy, raw.quote_ccy);
    let status = map_perp_status(&raw.status, &raw.symbol);
    let mut contract = Contract::new(settle, settlement);
    if let Some((scale, size)) = raw.contract_value.increment() {
        contract = contract.with_contract_size(scale, size);
    }

    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &['_']),
            raw.symbol,
            raw.base_ccy,
            raw.quote_ccy,
            InstrumentKind::Perpetual,
            contract,
        )
        .with_name(name)
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn map_spot_state(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "NORMAL" => InstrumentStatus::Trading,
        "PAUSE" | "POST_ONLY" => InstrumentStatus::Halted,
        "OFFLINE" | "DELISTED" => InstrumentStatus::Closed,
        other => {
            tracing::warn!(symbol, state = other, "unknown poloniex symbol state");
            InstrumentStatus::Unknown
        }
    }
}

fn map_perp_status(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "OPEN" => InstrumentStatus::Trading,
        "PAUSE" | "SUSPEND" => InstrumentStatus::Halted,
        "PENDING" => InstrumentStatus::PreOpen,
        "CLOSE" | "CLOSED" => InstrumentStatus::Closed,
        other => {
            tracing::warn!(symbol, status = other, "unknown poloniex contract status");
            InstrumentStatus::Unknown
        }
    }
}

/// Registers both Poloniex markets with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct PoloniexPlugin;

impl Plugin for PoloniexPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "poloniex".to_owned(),
            name: "Poloniex".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Poloniex spot and perpetual market data".to_owned(),
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
        let group = context.limit_group("poloniex");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(perp_source(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{map_spot_state, parse_perp, parse_spot};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const PERP: &[u8] = include_bytes!("../tests/fixtures/perp.json");

    #[test]
    fn spot_increments_come_from_the_nested_trade_limit() {
        let instruments = parse_spot(SPOT).unwrap();
        let first = instruments.first().expect("the fixture carries a pair");

        assert_eq!(first.kind, InstrumentKind::Spot);
        assert!(first.contract.is_none());
        // A scale of N means a tick of 1 at scale N.
        assert_eq!(first.tick_size, 1);
        assert_eq!(first.step_size, 1);
    }

    #[test]
    fn perpetuals_read_their_terse_keys() {
        let instruments = parse_perp(PERP).unwrap();
        let perp = instruments.first().expect("the fixture carries a contract");

        assert_eq!(perp.kind, InstrumentKind::Perpetual);
        assert!(!perp.base.is_empty() && !perp.quote.is_empty());
        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.expiry, None);
        assert!(contract.contract_size > 0);
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
        assert!(parse_perp(b"<html>nope</html>").is_err());
    }

    #[test]
    fn maps_documented_states() {
        assert_eq!(map_spot_state("NORMAL", "X"), InstrumentStatus::Trading);
        assert_eq!(map_spot_state("PAUSE", "X"), InstrumentStatus::Halted);
        assert_eq!(map_spot_state("weird", "X"), InstrumentStatus::Unknown);
    }
}
