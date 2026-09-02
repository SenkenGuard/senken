//! BitMart market data for Senken: spot pairs and futures contracts.
//!
//! Two hosts, two sources.
//!
//! Note that the spot endpoint returns far fewer pairs than BitMart lists
//! publicly — 65 rather than the ~1500 on its website. That is the venue's
//! answer, not a decoding failure: every spot path (`/symbols`,
//! `/symbols/details`, both hosts) and every User-Agent returns the same 65
//! with `code: 1000`, so BitMart considers it complete. It is a regional
//! restriction on the caller's network, and nothing this crate can widen.
//! The futures catalog is unaffected.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{Envelope, RawContract, RawSpot, Symbols};

mod api;
mod bars;
mod book;
mod feed;

pub use bars::{BitmartBarSource, bar_source};

/// Source id of the spot market.
pub const SPOT_ID: &str = "bitmart-spot";
/// Source id of the futures market.
pub const FUTURES_ID: &str = "bitmart-futures";

const SPOT_URL: &str = "https://api-cloud.bitmart.com/spot/v1/symbols/details";
const FUTURES_URL: &str = "https://api-cloud-v2.bitmart.com/contract/public/details";

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "BitMart Spot", SPOT_URL, client, parse_spot)
}

/// The futures market: perpetual and dated contracts.
#[must_use]
pub fn futures_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        FUTURES_ID,
        "BitMart Futures",
        FUTURES_URL,
        client,
        parse_futures,
    )
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<Vec<T>, SourceError> {
    let envelope: Envelope<Symbols<T>> =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    // BitMart answers 1000 on success.
    if envelope.code != 0 && envelope.code != 1000 {
        return Err(SourceError::rejected(format!(
            "code {}: {}",
            envelope.code, envelope.message
        )));
    }
    Ok(envelope.data.symbols)
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    Ok(decode::<RawSpot>(body)?
        .into_iter()
        .filter_map(spot_instrument)
        .collect())
}

fn spot_instrument(raw: RawSpot) -> Option<Instrument> {
    if raw.base_currency.is_empty() || raw.quote_currency.is_empty() {
        return skip(SPOT_ID, &raw.symbol, "missing base or quote currency");
    }
    // Spot gives a decimal place count for the price…
    let Some(price) = raw.price_max_precision.precision() else {
        return skip(SPOT_ID, &raw.symbol, "unusable price_max_precision");
    };
    // …and a real increment for the quantity.
    let Some(qty) = raw.quote_increment.increment() else {
        return skip(SPOT_ID, &raw.symbol, "unusable quote_increment");
    };

    let status = match raw.trade_status.as_str() {
        "trading" => InstrumentStatus::Trading,
        "pre-trade" => InstrumentStatus::PreOpen,
        _ => InstrumentStatus::Halted,
    };

    Some(
        Instrument::spot(
            normalise_symbol(&raw.symbol, &['_']),
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
    // `price_precision` is a decimal tick despite its name.
    let Some(price) = raw.price_precision.increment() else {
        return skip(FUTURES_ID, &raw.symbol, "unusable price_precision");
    };
    let Some(qty) = raw.vol_precision.increment() else {
        return skip(FUTURES_ID, &raw.symbol, "unusable vol_precision");
    };

    let expiry = raw.expire_timestamp.as_i64().filter(|ms| *ms > 0);
    let kind = if expiry.is_some() {
        InstrumentKind::Future
    } else {
        InstrumentKind::Perpetual
    };

    // BitMart marks its coin-margined contracts only by quoting them in
    // `USD`: `BTCUSD` settles in BTC and is worth 100 USD a contract,
    // while `BTCUSDT` settles in USDT. There is no explicit flag.
    let inverse = raw.quote_currency.eq_ignore_ascii_case("USD");
    let (settlement, settle) = if inverse {
        (Settlement::Inverse, raw.base_currency.clone())
    } else {
        (Settlement::Linear, raw.quote_currency.clone())
    };

    let mut contract = Contract::new(settle, settlement);
    if let Some(expiry) = expiry {
        let Some(expiry) = UnixNanos::from_millis(expiry) else {
            return skip(
                FUTURES_ID,
                &raw.symbol,
                "expire_timestamp overflowed UnixNanos",
            );
        };
        contract = contract.with_expiry(expiry);
    }
    if let Some((scale, size)) = raw.contract_size.increment() {
        contract = contract.with_contract_size(scale, size);
    }

    let status = if raw.status.eq_ignore_ascii_case("Trading") {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Halted
    };
    let name = match kind {
        InstrumentKind::Perpetual => {
            format!("{} / {} perpetual", raw.base_currency, raw.quote_currency)
        }
        _ => format!("{} / {} future", raw.base_currency, raw.quote_currency),
    };

    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &['_']),
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

/// Registers both BitMart markets with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct BitmartPlugin;

impl Plugin for BitmartPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "bitmart".to_owned(),
            name: "BitMart".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "BitMart spot and futures market data".to_owned(),
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
        let group = context.limit_group("bitmart");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(futures_source(client.clone())));
        // Spot only: BitMart's futures klines are not covered by this
        // source (see `bars`' own docs).
        context.register_bar_source(Arc::new(bar_source(
            client.clone(),
            Arc::new(senken_plugin::SystemClock),
        )));
        // Spot only, same as bars: the depth endpoint this source uses is
        // the v3 spot quotation host — see `book`'s own docs.
        context.register_book_source(Arc::new(crate::book::book_source(client)));
        context.register_feed_source(Arc::new(crate::feed::BitmartFeedSource::new()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_futures, parse_spot};
    use senken_marketdata::instrument::{InstrumentKind, Settlement};
    use senken_marketdata::source::SourceError;

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const FUTURES: &[u8] = include_bytes!("../tests/fixtures/futures.json");

    #[test]
    fn spot_mixes_a_precision_count_with_a_real_increment() {
        let instruments = parse_spot(SPOT).unwrap();
        let first = instruments
            .iter()
            .find(|i| i.source_symbol == "BTC_USDT")
            .expect("the fixture carries BTC_USDT");

        assert_eq!(first.kind, InstrumentKind::Spot);
        assert!(first.contract.is_none());
        assert_eq!(first.tick_size, 1, "the price tick comes from a count");
        assert_eq!(
            first.price_scale, 2,
            "BTC ticks in cents, not tens of dollars"
        );
    }

    #[test]
    fn a_futures_price_precision_is_actually_a_tick() {
        // BTCUSDT reports "0.1", which is a tick and not a digit count.
        let instruments = parse_futures(FUTURES).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();
        assert_eq!((btc.price_scale, btc.tick_size), (1, 1));
    }

    #[test]
    fn a_usdt_quoted_perpetual_is_linear_and_never_expires() {
        let instruments = parse_futures(FUTURES).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.symbol == "BTCUSDT")
            .expect("the fixture carries BTCUSDT");

        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.settle, "USDT");
        assert_eq!(contract.expiry, None);
    }

    #[test]
    fn a_usd_quoted_contract_is_inverse_though_no_field_says_so() {
        // BTCUSD settles in BTC and is worth 100 USD a contract; reading it
        // as linear would put the collateral in the wrong currency.
        let instruments = parse_futures(FUTURES).unwrap();
        let inverse = instruments
            .iter()
            .find(|i| i.symbol == "BTCUSD")
            .expect("the fixture carries BTCUSD");

        let contract = inverse.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Inverse);
        assert_eq!(contract.settle, "BTC");
        assert_eq!(contract.contract_size, 100);
    }

    #[test]
    fn a_failure_code_is_a_rejection() {
        let body = br#"{"code":30000,"message":"Not found","data":{}}"#;
        assert!(matches!(
            parse_spot(body),
            Err(SourceError::Rejected { .. })
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
        assert!(parse_futures(b"<html>nope</html>").is_err());
    }
}
