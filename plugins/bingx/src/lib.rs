//! BingX market data for Senken: spot, linear perpetuals and inverse
//! perpetuals.
//!
//! No BingX endpoint names the base and quote separately, so each pair is
//! split out of its symbol. The inverse market additionally demands a
//! `timestamp` query parameter even though the endpoint is public and
//! unsigned; the value is never validated, so a fixed one is sent.

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{Envelope, RawInverse, RawLinear, RawSpot, SpotData};

mod api;
mod bars;
mod book;
mod feed;

pub use bars::{BingxBarSource, bar_source_spot};

/// Source id of the spot market.
pub const SPOT_ID: &str = "bingx-spot";
/// Source id of the linear perpetual market.
pub const LINEAR_ID: &str = "bingx-linear";
/// Source id of the inverse perpetual market.
pub const INVERSE_ID: &str = "bingx-inverse";

const BASE_URL: &str = "https://open-api.bingx.com/openApi";
/// The inverse endpoint rejects a request with no `timestamp` at all, but
/// never checks the value, so any fixed one satisfies it.
const REQUIRED_TIMESTAMP: i64 = 1_700_000_000_000;

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        SPOT_ID,
        "BingX Spot",
        format!("{BASE_URL}/spot/v1/common/symbols"),
        client,
        parse_spot,
    )
}

/// The linear perpetual market.
#[must_use]
pub fn linear_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        LINEAR_ID,
        "BingX Linear",
        format!("{BASE_URL}/swap/v2/quote/contracts"),
        client,
        parse_linear,
    )
}

/// The inverse perpetual market.
#[must_use]
pub fn inverse_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        INVERSE_ID,
        "BingX Inverse",
        format!("{BASE_URL}/cswap/v1/market/contracts?timestamp={REQUIRED_TIMESTAMP}"),
        client,
        parse_inverse,
    )
}

fn check<T>(envelope: &Envelope<T>) -> Result<(), SourceError> {
    if envelope.code == 0 {
        return Ok(());
    }
    Err(SourceError::rejected(format!(
        "code {}: {}",
        envelope.code, envelope.msg
    )))
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let envelope: Envelope<SpotData> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    check(&envelope)?;
    Ok(envelope
        .data
        .symbols
        .into_iter()
        .filter_map(spot_instrument)
        .collect())
}

fn spot_instrument(raw: RawSpot) -> Option<Instrument> {
    let Some((base, quote)) = raw.symbol.split_once('-') else {
        return skip(SPOT_ID, &raw.symbol, "symbol has no separator");
    };
    let (base, quote) = (base.to_owned(), quote.to_owned());
    let Some(price) = raw.tick_size.increment() else {
        return skip(SPOT_ID, &raw.symbol, "unusable tickSize");
    };
    // `stepSize` arrives as `1e-06`; the shared decoder normalises it.
    let Some(qty) = raw.step_size.increment() else {
        return skip(SPOT_ID, &raw.symbol, "unusable stepSize");
    };

    Some(
        Instrument::spot(
            normalise_symbol(&raw.symbol, &['-']),
            raw.symbol,
            base,
            quote,
        )
        .with_status(online(raw.status))
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn parse_linear(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let envelope: Envelope<Vec<RawLinear>> =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    check(&envelope)?;
    Ok(envelope
        .data
        .into_iter()
        .filter_map(linear_instrument)
        .collect())
}

fn linear_instrument(raw: RawLinear) -> Option<Instrument> {
    if raw.asset.is_empty() || raw.currency.is_empty() {
        return skip(LINEAR_ID, &raw.symbol, "missing asset or currency");
    }
    // This market publishes decimal places, never a tick size.
    let Some(price) = raw.price_precision.precision() else {
        return skip(LINEAR_ID, &raw.symbol, "unusable pricePrecision");
    };
    let Some(qty) = raw.quantity_precision.precision() else {
        return skip(LINEAR_ID, &raw.symbol, "unusable quantityPrecision");
    };

    let mut contract = Contract::new(&raw.currency, Settlement::Linear);
    if let Some((scale, size)) = raw.size.increment() {
        contract = contract.with_contract_size(scale, size);
    }

    let name = format!("{} / {} perpetual", raw.asset, raw.currency);
    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &['-']),
            raw.symbol,
            raw.asset,
            raw.currency,
            InstrumentKind::Perpetual,
            contract,
        )
        .with_name(name)
        .with_status(online(raw.status))
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn parse_inverse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let envelope: Envelope<Vec<RawInverse>> =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    check(&envelope)?;
    Ok(envelope
        .data
        .into_iter()
        .filter_map(inverse_instrument)
        .collect())
}

fn inverse_instrument(raw: RawInverse) -> Option<Instrument> {
    let Some((base, quote)) = raw.symbol.split_once('-') else {
        return skip(INVERSE_ID, &raw.symbol, "symbol has no separator");
    };
    let (base, quote) = (base.to_owned(), quote.to_owned());
    // `minTickSize` is the per-contract notional, not a tick; the decimal
    // places are the only honest description of the price increment.
    let Some(price) = raw.price_precision.precision() else {
        return skip(INVERSE_ID, &raw.symbol, "unusable pricePrecision");
    };

    // Inverse quantities are whole contracts.
    let mut contract = Contract::new(&base, Settlement::Inverse);
    if let Some((scale, size)) = raw.min_trade_value.increment() {
        contract = contract.with_contract_size(scale, size);
    }

    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &['-']),
            raw.symbol,
            base.clone(),
            quote.clone(),
            InstrumentKind::Perpetual,
            contract,
        )
        .with_name(format!("{base} / {quote} perpetual"))
        .with_status(online(raw.status))
        .with_price_increment(price)
        .with_qty_increment((0, 1)),
    )
}

/// BingX reports trading state as an integer, `1` meaning open.
fn online(status: i64) -> InstrumentStatus {
    match status {
        1 => InstrumentStatus::Trading,
        0 => InstrumentStatus::Closed,
        _ => InstrumentStatus::Halted,
    }
}

/// Registers every BingX market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct BingxPlugin;

impl Plugin for BingxPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "bingx".to_owned(),
            name: "BingX".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "BingX spot, linear and inverse perpetual market data".to_owned(),
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
        let group = context.limit_group("bingx");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(linear_source(client.clone())));
        context.register_marketdata_source(Arc::new(inverse_source(client.clone())));
        context.register_bar_source(Arc::new(bar_source_spot(client.clone())));
        // Depth, declared the same way as everything above rather than
        // wired into the HTTP layer by hand.
        context.register_book_source(Arc::new(crate::book::book_source(SPOT_ID, client)));
        context.register_feed_source(Arc::new(crate::feed::BingxFeedSource::new()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{inverse_source, parse_inverse, parse_linear, parse_spot};
    use senken_marketdata::MarketDataSource;
    use senken_marketdata::instrument::{InstrumentKind, Settlement};
    use senken_marketdata::source::SourceError;
    use senken_venue::{LimitGroup, VenueClient};

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const LINEAR: &[u8] = include_bytes!("../tests/fixtures/linear.json");
    const INVERSE: &[u8] = include_bytes!("../tests/fixtures/inverse.json");

    #[test]
    fn a_spot_step_in_scientific_notation_is_read_correctly() {
        // BingX sends BTC-USDT's step as 1e-06.
        let instruments = parse_spot(SPOT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USDT"));
        assert_eq!((btc.price_scale, btc.tick_size), (2, 1));
        assert_eq!((btc.qty_scale, btc.step_size), (6, 1));
    }

    #[test]
    fn linear_perpetuals_settle_in_their_currency_field() {
        let instruments = parse_linear(LINEAR).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!(btc.kind, InstrumentKind::Perpetual);
        let contract = btc.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.settle, "USDT");
        assert_eq!(contract.expiry, None);
    }

    #[test]
    fn the_inverse_tick_never_comes_from_min_tick_size() {
        // `minTickSize` on BTC-USD is 100 — a USD notional, not a tick.
        let instruments = parse_inverse(INVERSE).unwrap();
        let any = instruments.first().expect("the fixture carries a contract");

        assert!(
            any.tick_size == 1,
            "the tick must be 1 at the reported scale, not a notional"
        );
        assert_eq!(
            any.contract.as_ref().unwrap().settlement,
            Settlement::Inverse
        );
        assert_eq!(any.contract.as_ref().unwrap().settle, any.base);
    }

    #[test]
    fn the_inverse_url_carries_the_timestamp_the_venue_demands() {
        let source = inverse_source(test_client());
        assert!(source.url().contains("timestamp="), "{}", source.url());
        assert_eq!(source.id(), "bingx-inverse");
    }

    #[test]
    fn a_failure_code_is_a_rejection() {
        let body = br#"{"code":100400,"msg":"Invalid parameter","data":[]}"#;
        assert!(matches!(
            parse_linear(body),
            Err(SourceError::Rejected { reason }) if reason.contains("100400")
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
        assert!(parse_linear(b"<html>nope</html>").is_err());
    }
}
