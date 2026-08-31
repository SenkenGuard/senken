//! BitMEX market data for Senken: perpetuals — linear, inverse and quanto —
//! and dated futures.
//!
//! BitMEX is the venue that makes [`Settlement::Quanto`] necessary: an
//! `ETH/USD` contract margined in Bitcoin is neither linear nor inverse,
//! because the collateral floats against both legs. Price indices are
//! listed in the same document and are left out, being untradable.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, iso8601_ms, normalise_symbol, skip};

use crate::api::RawInstrument;

mod api;

/// Source id of the BitMEX market.
pub const SOURCE_ID: &str = "bitmex";

const URL: &str = "https://www.bitmex.com/api/v1/instrument/active";

/// Every live BitMEX contract.
#[must_use]
pub fn source(client: VenueClient) -> HttpSource {
    HttpSource::new(SOURCE_ID, "BitMEX", URL, client, parse)
}

fn parse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let raw: Vec<RawInstrument> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(raw.into_iter().filter_map(to_instrument).collect())
}

fn to_instrument(raw: RawInstrument) -> Option<Instrument> {
    // `IFXXXP` rows are reference indices, not contracts anyone can trade.
    if raw.instrument_type.starts_with("IF") {
        return None;
    }
    if raw.underlying.is_empty() || raw.quote_currency.is_empty() {
        return skip(SOURCE_ID, &raw.symbol, "missing underlying or quote");
    }
    let Some(price) = raw.tick_size.increment() else {
        return skip(SOURCE_ID, &raw.symbol, "unusable tickSize");
    };
    let Some(qty) = raw.lot_size.increment() else {
        return skip(SOURCE_ID, &raw.symbol, "unusable lotSize");
    };

    // The key is simply absent on a perpetual, never null or a sentinel.
    let expiry = raw.expiry.as_deref().and_then(iso8601_ms);
    let kind = if expiry.is_some() {
        InstrumentKind::Future
    } else {
        InstrumentKind::Perpetual
    };

    let settlement = if raw.is_quanto {
        Settlement::Quanto
    } else if raw.is_inverse {
        Settlement::Inverse
    } else {
        Settlement::Linear
    };
    let settle = settle_currency(&raw);

    let mut contract = Contract::new(settle, settlement);
    if let Some(expiry) = expiry {
        let Some(expiry) = UnixNanos::from_millis(expiry) else {
            return skip(SOURCE_ID, &raw.symbol, "expiry overflowed UnixNanos");
        };
        contract = contract.with_expiry(expiry);
    }
    // The multiplier's sign encodes inverseness, which `settlement`
    // already records; only its magnitude is a contract size.
    if let Some((scale, size)) = raw.multiplier.increment() {
        contract = contract.with_contract_size(scale, size.abs());
    }

    let status = map_status(&raw.state, &raw.symbol);
    let name = match kind {
        InstrumentKind::Perpetual => {
            format!("{} / {} perpetual", raw.underlying, raw.quote_currency)
        }
        _ => format!("{} / {} future", raw.underlying, raw.quote_currency),
    };

    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &['_']),
            raw.symbol,
            raw.underlying,
            raw.quote_currency,
            kind,
            contract,
        )
        .with_name(name)
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

/// BitMEX names settlement currencies in their smallest unit — `XBt` is
/// satoshis of `XBT`, `USDt` is micro-`USDT` — so upper-casing recovers the
/// currency itself.
fn settle_currency(raw: &RawInstrument) -> String {
    if raw.settl_currency.is_empty() {
        raw.quote_currency.to_uppercase()
    } else {
        raw.settl_currency.to_uppercase()
    }
}

fn map_status(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "Open" => InstrumentStatus::Trading,
        "Unlisted" => InstrumentStatus::PreOpen,
        "Settled" | "Delisted" | "Closed" => InstrumentStatus::Closed,
        other => {
            tracing::warn!(symbol, state = other, "unknown bitmex instrument state");
            InstrumentStatus::Unknown
        }
    }
}

/// Registers the BitMEX market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct BitmexPlugin;

impl Plugin for BitmexPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "bitmex".to_owned(),
            name: "BitMEX".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "BitMEX perpetual, quanto and dated futures market data".to_owned(),
            permissions: Vec::new(),
        }
    }

    fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        let group = context.limit_group("bitmex");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(source(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{map_status, parse, settle_currency};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/instruments.json");

    fn settlements() -> Vec<Settlement> {
        parse(FIXTURE)
            .unwrap()
            .iter()
            .filter_map(|i| i.contract.as_ref().map(|c| c.settlement))
            .collect()
    }

    #[test]
    fn an_inverse_perpetual_has_no_expiry() {
        let instruments = parse(FIXTURE).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.symbol == "XBTUSD")
            .expect("the fixture carries XBTUSD");

        assert_eq!(perp.kind, InstrumentKind::Perpetual);
        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Inverse);
        assert_eq!(
            contract.expiry, None,
            "BitMEX omits the expiry key entirely on a perpetual"
        );
        assert_eq!(contract.settle, "XBT", "XBt is satoshis of XBT");
    }

    #[test]
    fn a_quanto_contract_is_neither_linear_nor_inverse() {
        assert!(
            settlements().contains(&Settlement::Quanto),
            "the fixture carries a quanto contract"
        );
    }

    #[test]
    fn dated_futures_carry_their_expiry() {
        let instruments = parse(FIXTURE).unwrap();
        let dated = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Future)
            .expect("the fixture carries a dated future");
        assert!(dated.contract.as_ref().unwrap().expiry.is_some());
    }

    #[test]
    fn price_indices_are_not_instruments() {
        let raw: Vec<super::RawInstrument> = serde_json::from_slice(FIXTURE).unwrap();
        let indices: Vec<&str> = raw
            .iter()
            .filter(|r| r.instrument_type.starts_with("IF"))
            .map(|r| r.symbol.as_str())
            .collect();
        assert!(!indices.is_empty(), "the fixture carries a price index");

        let instruments = parse(FIXTURE).unwrap();
        assert!(
            instruments
                .iter()
                .all(|i| !indices.contains(&i.source_symbol.as_str())),
            "reference indices are untradable and must be left out"
        );
    }

    #[test]
    fn a_contract_size_is_never_negative() {
        // BitMEX signs the multiplier to mark inverse contracts.
        let instruments = parse(FIXTURE).unwrap();
        assert!(
            instruments
                .iter()
                .filter_map(|i| i.contract.as_ref())
                .all(|c| c.contract_size > 0)
        );
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse(b"<html>nope</html>").is_err());
    }

    #[test]
    fn maps_documented_states() {
        assert_eq!(map_status("Open", "X"), InstrumentStatus::Trading);
        assert_eq!(map_status("Settled", "X"), InstrumentStatus::Closed);
        assert_eq!(map_status("Whatever", "X"), InstrumentStatus::Unknown);
    }

    #[test]
    fn settlement_currencies_lose_their_smallest_unit_spelling() {
        let raw: Vec<super::RawInstrument> = serde_json::from_slice(FIXTURE).unwrap();
        for instrument in &raw {
            let settle = settle_currency(instrument);
            assert_eq!(settle, settle.to_uppercase());
        }
    }
}
