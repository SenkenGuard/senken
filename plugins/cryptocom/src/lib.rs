//! Crypto.com Exchange market data for Senken: spot, perpetual swaps and
//! dated futures.
//!
//! One endpoint returns all three, so this is one source; `inst_type` is
//! what tells them apart. Every derivative here is USD-settled and linear —
//! the venue lists no inverse contracts — and the same document also
//! carries equity, commodity and pre-IPO products alongside crypto.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{InstrumentsResponse, RawInstrument};

mod api;
mod bars;
mod book;
mod feed;

pub use bars::{CryptocomBarSource, bar_source};

/// Source id of the Crypto.com market.
pub const SOURCE_ID: &str = "cryptocom";

const URL: &str = "https://api.crypto.com/exchange/v1/public/get-instruments";

/// Every Crypto.com instrument: spot, perpetual and dated.
#[must_use]
pub fn source(client: VenueClient) -> HttpSource {
    HttpSource::new(SOURCE_ID, "Crypto.com", URL, client, parse)
}

fn parse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let response: InstrumentsResponse =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    if response.code != 0 {
        return Err(SourceError::rejected(format!(
            "code {}: {}",
            response.code, response.message
        )));
    }
    Ok(response
        .result
        .data
        .into_iter()
        .filter_map(to_instrument)
        .collect())
}

fn to_instrument(raw: RawInstrument) -> Option<Instrument> {
    let kind = match raw.inst_type.as_str() {
        "CCY_PAIR" => InstrumentKind::Spot,
        "PERPETUAL_SWAP" => InstrumentKind::Perpetual,
        "FUTURE" => InstrumentKind::Future,
        other => return skip(SOURCE_ID, &raw.symbol, other),
    };

    if raw.base_ccy.is_empty() || raw.quote_ccy.is_empty() {
        return skip(SOURCE_ID, &raw.symbol, "missing base or quote currency");
    }
    let Some(price) = raw.price_tick_size.increment() else {
        return skip(SOURCE_ID, &raw.symbol, "unusable price_tick_size");
    };
    let Some(qty) = raw.qty_tick_size.increment() else {
        return skip(SOURCE_ID, &raw.symbol, "unusable qty_tick_size");
    };

    let status = if raw.tradable {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Halted
    };
    let symbol = normalise_symbol(&raw.symbol, &['_', '-']);

    let instrument = if kind == InstrumentKind::Spot {
        Instrument::spot(symbol, raw.symbol, raw.base_ccy, &raw.quote_ccy)
    } else {
        // Crypto.com lists no inverse contracts; everything settles in the
        // quote currency.
        let mut contract = Contract::new(&raw.quote_ccy, Settlement::Linear);
        if let Some(expiry) = raw.expiry_timestamp_ms.as_i64().filter(|ms| *ms > 0) {
            let Some(expiry) = UnixNanos::from_millis(expiry) else {
                return skip(
                    SOURCE_ID,
                    &raw.symbol,
                    "expiry_timestamp_ms overflowed UnixNanos",
                );
            };
            contract = contract.with_expiry(expiry);
        }
        if let Some((scale, size)) = raw.contract_size.increment() {
            contract = contract.with_contract_size(scale, size);
        }

        let name = match kind {
            InstrumentKind::Perpetual => format!("{} / {} perpetual", raw.base_ccy, raw.quote_ccy),
            _ => format!("{} / {} future", raw.base_ccy, raw.quote_ccy),
        };
        Instrument::derivative(
            symbol,
            raw.symbol,
            raw.base_ccy,
            raw.quote_ccy,
            kind,
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

/// Registers the Crypto.com market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct CryptocomPlugin;

impl Plugin for CryptocomPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "cryptocom".to_owned(),
            name: "Crypto.com".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Crypto.com spot, perpetual and futures market data".to_owned(),
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
        let group = context.limit_group("cryptocom");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(source(client.clone())));
        context.register_bar_source(Arc::new(bar_source(
            client.clone(),
            Arc::new(senken_plugin::SystemClock),
        )));
        context.register_book_source(Arc::new(crate::book::book_source(client)));
        context.register_feed_source(Arc::new(crate::feed::CryptocomFeedSource::new()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use senken_marketdata::instrument::{InstrumentKind, Settlement};
    use senken_marketdata::source::SourceError;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/instruments.json");

    #[test]
    fn one_document_carries_all_three_kinds() {
        let instruments = parse(FIXTURE).unwrap();
        for kind in [
            InstrumentKind::Spot,
            InstrumentKind::Perpetual,
            InstrumentKind::Future,
        ] {
            assert!(
                instruments.iter().any(|i| i.kind == kind),
                "the fixture carries a {kind:?}"
            );
        }
    }

    #[test]
    fn every_derivative_is_linear_and_quote_settled() {
        let instruments = parse(FIXTURE).unwrap();
        for derivative in instruments.iter().filter(|i| i.kind.is_derivative()) {
            let contract = derivative.contract.as_ref().unwrap();
            assert_eq!(contract.settlement, Settlement::Linear);
            assert_eq!(contract.settle, derivative.quote);
        }
    }

    #[test]
    fn only_a_dated_future_carries_an_expiry() {
        let instruments = parse(FIXTURE).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Perpetual)
            .unwrap();
        assert_eq!(perp.contract.as_ref().unwrap().expiry, None);

        let dated = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Future)
            .unwrap();
        assert!(dated.contract.as_ref().unwrap().expiry.is_some());
    }

    #[test]
    fn an_integer_error_code_is_a_rejection() {
        // v1 reports the code as a number; v2 used a string.
        let body = br#"{"code":10004,"message":"BAD_REQUEST","result":{"data":[]}}"#;
        assert!(matches!(
            parse(body),
            Err(SourceError::Rejected { reason }) if reason.contains("10004")
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse(b"<html>nope</html>").is_err());
    }
}
