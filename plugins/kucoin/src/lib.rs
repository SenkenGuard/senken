//! KuCoin market data for Senken: spot pairs and futures contracts.
//!
//! The two live on different hosts. Futures cover perpetuals and dated
//! contracts, linear and inverse, in one document; `isInverse` is the flag
//! to trust, never the sign of `multiplier`.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{Envelope, RawContract, RawSymbol};

mod api;
mod bars;
mod book;
mod feed;

pub use bars::{KucoinBarSource, bar_source_spot};

/// Source id of the spot market.
pub const SPOT_ID: &str = "kucoin-spot";
/// Source id of the futures market.
pub const FUTURES_ID: &str = "kucoin-futures";

const SPOT_URL: &str = "https://api.kucoin.com/api/v2/symbols";
const FUTURES_URL: &str = "https://api-futures.kucoin.com/api/v1/contracts/active";
/// KuCoin's success code, as a string on both hosts.
const OK: &str = "200000";

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "KuCoin Spot", SPOT_URL, client, parse_spot)
}

/// The futures market: perpetual and dated, linear and inverse.
#[must_use]
pub fn futures_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        FUTURES_ID,
        "KuCoin Futures",
        FUTURES_URL,
        client,
        parse_futures,
    )
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<Vec<T>, SourceError> {
    let envelope: Envelope<T> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    if !envelope.code.is_empty() && envelope.code != OK {
        return Err(SourceError::rejected(format!(
            "code {}: {}",
            envelope.code, envelope.msg
        )));
    }
    Ok(envelope.data)
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    Ok(decode::<RawSymbol>(body)?
        .into_iter()
        .filter_map(spot_instrument)
        .collect())
}

fn spot_instrument(raw: RawSymbol) -> Option<Instrument> {
    if raw.base_currency.is_empty() || raw.quote_currency.is_empty() {
        return skip(SPOT_ID, &raw.symbol, "missing base or quote currency");
    }
    let Some(price) = raw.price_increment.increment() else {
        return skip(SPOT_ID, &raw.symbol, "unusable priceIncrement");
    };
    let Some(qty) = raw.base_increment.increment() else {
        return skip(SPOT_ID, &raw.symbol, "unusable baseIncrement");
    };

    let status = if raw.enable_trading {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Halted
    };

    Some(
        Instrument::spot(
            normalise_symbol(&raw.symbol, &['-']),
            raw.symbol,
            raw.base_currency,
            raw.quote_currency,
        )
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn parse_futures(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    Ok(decode::<RawContract>(body)?
        .into_iter()
        .filter_map(futures_instrument)
        .collect())
}

fn futures_instrument(raw: RawContract) -> Option<Instrument> {
    if raw.base_currency.is_empty() || raw.quote_currency.is_empty() {
        return skip(FUTURES_ID, &raw.symbol, "missing base or quote currency");
    }
    let Some(price) = raw.tick_size.increment() else {
        return skip(FUTURES_ID, &raw.symbol, "unusable tickSize");
    };
    let Some(qty) = raw.lot_size.increment() else {
        return skip(FUTURES_ID, &raw.symbol, "unusable lotSize");
    };

    let settlement = if raw.is_inverse {
        Settlement::Inverse
    } else {
        Settlement::Linear
    };
    let settle = if raw.settle_currency.is_empty() {
        raw.quote_currency.as_str()
    } else {
        raw.settle_currency.as_str()
    };

    let expiry = raw.expire_date.filter(|ms| *ms > 0);
    let kind = if expiry.is_some() {
        InstrumentKind::Future
    } else {
        InstrumentKind::Perpetual
    };

    let mut contract = Contract::new(settle, settlement);
    if let Some(expiry) = expiry {
        let Some(expiry) = UnixNanos::from_millis(expiry) else {
            return skip(FUTURES_ID, &raw.symbol, "expireDate overflowed UnixNanos");
        };
        contract = contract.with_expiry(expiry);
    }
    // The multiplier is negative on inverse contracts; `isInverse` already
    // records that, so only the magnitude is a contract size.
    if let Some((scale, size)) = raw.multiplier.increment() {
        contract = contract.with_contract_size(scale, size.abs());
    }

    let status = map_contract_status(&raw.status, &raw.symbol);
    let name = match kind {
        InstrumentKind::Perpetual => {
            format!("{} / {} perpetual", raw.base_currency, raw.quote_currency)
        }
        _ => format!("{} / {} future", raw.base_currency, raw.quote_currency),
    };

    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &['-']),
            raw.symbol,
            raw.base_currency,
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

fn map_contract_status(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "Open" => InstrumentStatus::Trading,
        "Pause" | "BeingSettled" => InstrumentStatus::Halted,
        "Init" => InstrumentStatus::PreOpen,
        "Closed" | "CancelOnly" => InstrumentStatus::Closed,
        other => {
            tracing::warn!(symbol, status = other, "unknown kucoin contract status");
            InstrumentStatus::Unknown
        }
    }
}

/// Registers both KuCoin markets with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct KucoinPlugin;

impl Plugin for KucoinPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "kucoin".to_owned(),
            name: "KuCoin".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "KuCoin spot and futures market data".to_owned(),
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
        let group = context.limit_group("kucoin");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(futures_source(client.clone())));
        context.register_bar_source(Arc::new(bar_source_spot(
            client.clone(),
            Arc::new(senken_plugin::SystemClock),
        )));
        // Depth, declared the same way as everything above rather than
        // wired into the HTTP layer by hand.
        context.register_book_source(Arc::new(crate::book::book_source(SPOT_ID, client.clone())));
        // The live feed fetches its own WebSocket token over HTTP before
        // every dial, so it needs the same rate-limited client as the rest.
        context.register_feed_source(Arc::new(crate::feed::KucoinFeedSource::new(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{map_contract_status, parse_futures, parse_spot};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};
    use senken_marketdata::source::SourceError;

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const FUTURES: &[u8] = include_bytes!("../tests/fixtures/futures.json");

    #[test]
    fn spot_pairs_normalise_their_dash() {
        let instruments = parse_spot(SPOT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!(btc.source_symbol, "BTC-USDT");
        assert_eq!(btc.kind, InstrumentKind::Spot);
        assert!(btc.contract.is_none());
    }

    #[test]
    fn futures_numbers_arrive_as_json_numbers_not_strings() {
        // KuCoin futures send tickSize/lotSize/multiplier unquoted.
        let instruments = parse_futures(FUTURES).unwrap();
        assert!(!instruments.is_empty());
        assert!(instruments.iter().all(|i| i.tick_size >= 1));
    }

    #[test]
    fn inverse_contracts_are_flagged_not_inferred_from_the_multiplier() {
        let instruments = parse_futures(FUTURES).unwrap();
        if let Some(inverse) = instruments
            .iter()
            .find(|i| i.contract.as_ref().unwrap().settlement == Settlement::Inverse)
        {
            assert!(
                inverse.contract.as_ref().unwrap().contract_size > 0,
                "the multiplier's sign encodes inverseness, not a size"
            );
        }
    }

    #[test]
    fn a_perpetual_has_no_expire_date() {
        let instruments = parse_futures(FUTURES).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Perpetual)
            .expect("the fixture carries a perpetual");
        assert_eq!(perp.contract.as_ref().unwrap().expiry, None);
    }

    #[test]
    fn a_failure_code_is_a_rejection() {
        let body = br#"{"code":"400100","msg":"Invalid parameter","data":[]}"#;
        assert!(matches!(
            parse_spot(body),
            Err(SourceError::Rejected { reason }) if reason.contains("400100")
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
        assert!(parse_futures(b"<html>nope</html>").is_err());
    }

    #[test]
    fn maps_documented_statuses() {
        assert_eq!(map_contract_status("Open", "X"), InstrumentStatus::Trading);
        assert_eq!(map_contract_status("Pause", "X"), InstrumentStatus::Halted);
        assert_eq!(map_contract_status("weird", "X"), InstrumentStatus::Unknown);
    }
}
