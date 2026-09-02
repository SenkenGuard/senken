//! Kraken market data for Senken: spot pairs and Kraken Futures.
//!
//! Two products, two shapes. Spot answers with an object keyed by pair
//! name and carries Kraken's legacy asset codes (`XXBT`, `ZUSD`), which are
//! normalised here. Futures answer with an array covering perpetuals and
//! dated contracts, linear and inverse together.
//!
//! # No live feed, and exactly why
//!
//! Kraken's stream and its REST API disagree about what an instrument is
//! called, and the catalog can only remember one of the two names.
//! Confirmed live, 2026-09-02:
//!
//! ```text
//! REST  /0/public/Depth?pair=XBTUSD  → the book
//! REST  /0/public/Depth?pair=XBT/USD → {"error":["EQuery:Unknown asset pair"]}
//! WS v2 subscribe symbol "XBTUSD"    → "Currency pair not in ISO 4217-A3 format"
//! WS v2 subscribe symbol "XBT/USD"   → "Currency pair not supported"
//! WS v2 subscribe symbol "BTC/USD"   → subscribed
//! WS v1 subscribe pair   "BTC/USD"   → subscribed (echoed back as XBT/USD)
//! ```
//!
//! Instruments, bars and depth all need `altname` (`XBTUSD`), which is
//! what this plugin stores as `source_symbol` — and the stream needs a
//! slashed ISO form that is not derivable from it, because which
//! characters were separators is precisely what normalising removed.
//! `SymbolMap` resolves one symbol per instrument, so there is nowhere to
//! put the second.
//!
//! Registering a feed anyway would mean guessing where to split `XBTUSD`,
//! which is the kind of invented venue fact this project's fixtures exist
//! to prevent. The fix is for the instrument catalog to carry a venue's
//! *stream* symbol beside its *request* symbol — a `senken-marketdata`
//! contract change, worth making deliberately rather than smuggling in
//! here.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::decimal::increment_from_precision;
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, iso8601_ms, normalise_symbol, skip};

use crate::api::{AssetPairsResponse, InstrumentsResponse, RawInstrument, RawPair};

mod api;
mod bars;
mod book;

pub use bars::{KrakenBarSource, bar_source_spot};

/// Source id of the spot market.
pub const SPOT_ID: &str = "kraken-spot";
/// Source id of the futures market.
pub const FUTURES_ID: &str = "kraken-futures";

const SPOT_URL: &str = "https://api.kraken.com/0/public/AssetPairs";
const FUTURES_URL: &str = "https://futures.kraken.com/derivatives/api/v3/instruments";

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "Kraken Spot", SPOT_URL, client, parse_spot)
}

/// The futures market: perpetuals and dated contracts, linear and inverse.
#[must_use]
pub fn futures_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        FUTURES_ID,
        "Kraken Futures",
        FUTURES_URL,
        client,
        parse_futures,
    )
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let response: AssetPairsResponse = serde_json::from_slice(body).map_err(SourceError::decode)?;
    if let Some(first) = response.error.first() {
        // Kraken reports argument errors inside an HTTP 200 body.
        return Err(SourceError::rejected(first.clone()));
    }
    Ok(response
        .result
        .into_iter()
        .filter_map(|(key, pair)| spot_instrument(&key, pair))
        .collect())
}

fn spot_instrument(key: &str, raw: RawPair) -> Option<Instrument> {
    // Kraken keys its map by a canonical name and repeats a friendlier one
    // in `altname`; either can be the venue symbol.
    let source_symbol = if raw.altname.is_empty() {
        key.to_owned()
    } else {
        raw.altname
    };
    let Some((base, quote)) = spot_pair(&raw.wsname, &raw.base, &raw.quote) else {
        return skip(SPOT_ID, &source_symbol, "no usable base/quote");
    };
    let Some(price) = raw.tick_size.increment() else {
        return skip(SPOT_ID, &source_symbol, "unusable tick_size");
    };
    // Spot publishes only a decimal-place count for quantities.
    let Some(qty) = raw.lot_decimals.precision() else {
        return skip(SPOT_ID, &source_symbol, "unusable lot_decimals");
    };

    let status = map_spot_status(&raw.status, &source_symbol);
    Some(
        Instrument::spot(
            normalise_symbol(&source_symbol, &['/']),
            source_symbol,
            base,
            quote,
        )
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

/// Base and quote, preferring `wsname` (`XBT/USD`) because `base`/`quote`
/// carry Kraken's legacy prefixes.
fn spot_pair(wsname: &str, base: &str, quote: &str) -> Option<(String, String)> {
    if let Some((base, quote)) = wsname.split_once('/') {
        return Some((base.to_owned(), quote.to_owned()));
    }
    if base.is_empty() || quote.is_empty() {
        return None;
    }
    Some((strip_legacy_prefix(base), strip_legacy_prefix(quote)))
}

/// Kraken's older four-character asset codes carry an `X` (crypto) or `Z`
/// (fiat) prefix: `XXBT` is XBT, `ZUSD` is USD. Newer codes do not.
fn strip_legacy_prefix(code: &str) -> String {
    if code.len() == 4 && (code.starts_with('X') || code.starts_with('Z')) {
        code[1..].to_owned()
    } else {
        code.to_owned()
    }
}

fn parse_futures(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let response: InstrumentsResponse =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    if !response.result.is_empty() && response.result != "success" {
        return Err(SourceError::rejected(format!(
            "{}: {}",
            response.result, response.error
        )));
    }
    Ok(response
        .instruments
        .into_iter()
        .filter_map(futures_instrument)
        .collect())
}

fn futures_instrument(raw: RawInstrument) -> Option<Instrument> {
    if raw.base.is_empty() || raw.quote.is_empty() {
        return skip(FUTURES_ID, &raw.symbol, "missing base or quote");
    }
    let Some(price) = raw.tick_size.increment() else {
        return skip(FUTURES_ID, &raw.symbol, "unusable tickSize");
    };
    let Some(qty) = quantity_increment(&raw) else {
        return skip(FUTURES_ID, &raw.symbol, "unusable quantity precision");
    };

    let settlement = if raw.instrument_type.contains("inverse") {
        Settlement::Inverse
    } else {
        Settlement::Linear
    };
    let settle = if settlement == Settlement::Inverse {
        raw.base.as_str()
    } else {
        raw.quote.as_str()
    };

    // Only dated contracts carry a last trading time; its absence is what
    // marks a perpetual.
    let expiry = iso8601_ms(&raw.last_trading_time);
    let kind = if expiry.is_some() {
        InstrumentKind::Future
    } else {
        InstrumentKind::Perpetual
    };

    let mut contract = Contract::new(settle, settlement);
    if let Some(expiry) = expiry {
        let Some(expiry) = UnixNanos::from_millis(expiry) else {
            return skip(
                FUTURES_ID,
                &raw.symbol,
                "lastTradingTime overflowed UnixNanos",
            );
        };
        contract = contract.with_expiry(expiry);
    }
    if let Some((scale, size)) = raw.contract_size.increment() {
        contract = contract.with_contract_size(scale, size);
    }

    let status = if raw.is_expired {
        InstrumentStatus::Closed
    } else if raw.tradeable {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Halted
    };

    let name = match kind {
        InstrumentKind::Perpetual => format!("{} / {} perpetual", raw.base, raw.quote),
        _ => format!("{} / {} future", raw.base, raw.quote),
    };

    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &['/']),
            raw.symbol,
            raw.base,
            raw.quote,
            kind,
            contract,
        )
        .with_name(name)
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

/// Kraken Futures reports quantity precision as decimal places that **may
/// be negative**: `-3` means the step is 1000 whole units, not a fraction.
fn quantity_increment(raw: &RawInstrument) -> Option<(u8, i64)> {
    let digits = raw.contract_value_trade_precision.as_i64()?;
    if digits >= 0 {
        return Some(increment_from_precision(u32::try_from(digits).ok()?));
    }
    let step = 10_i64.checked_pow(u32::try_from(-digits).ok()?)?;
    Some((0, step))
}

fn map_spot_status(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "online" => InstrumentStatus::Trading,
        "cancel_only" | "post_only" | "limit_only" | "reduce_only" => InstrumentStatus::Halted,
        other => {
            tracing::warn!(symbol, status = other, "unknown kraken pair status");
            InstrumentStatus::Unknown
        }
    }
}

/// Registers both Kraken markets with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct KrakenPlugin;

impl Plugin for KrakenPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "kraken".to_owned(),
            name: "Kraken".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Kraken spot and futures market data".to_owned(),
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
        let group = context.limit_group("kraken");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(futures_source(client.clone())));
        // Spot only: the futures OHLC endpoint has its own shape and was
        // not covered this session — see `bars`' own module docs.
        context.register_bar_source(Arc::new(bar_source_spot(
            client.clone(),
            Arc::new(senken_plugin::SystemClock),
        )));
        context.register_book_source(Arc::new(crate::book::book_source_spot(
            client.clone(),
            Arc::new(senken_plugin::SystemClock),
        )));
        context.register_book_source(Arc::new(crate::book::book_source_futures(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{map_spot_status, parse_futures, parse_spot, strip_legacy_prefix};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};
    use senken_marketdata::source::SourceError;

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const FUTURES: &[u8] = include_bytes!("../tests/fixtures/futures.json");

    #[test]
    fn spot_pairs_come_out_of_an_object_not_an_array() {
        let instruments = parse_spot(SPOT).unwrap();
        assert!(!instruments.is_empty());

        let btc = instruments.iter().find(|i| i.base == "XBT").unwrap();
        assert_eq!(btc.quote, "USD");
        assert_eq!(btc.kind, InstrumentKind::Spot);
        assert!(btc.tick_size >= 1 && btc.step_size >= 1);
    }

    #[test]
    fn legacy_asset_codes_lose_their_prefix() {
        assert_eq!(strip_legacy_prefix("XXBT"), "XBT");
        assert_eq!(strip_legacy_prefix("ZUSD"), "USD");
        assert_eq!(strip_legacy_prefix("USDT"), "USDT", "four chars, no prefix");
        assert_eq!(strip_legacy_prefix("SOL"), "SOL");
    }

    #[test]
    fn an_error_array_is_a_rejection() {
        let body = br#"{"error":["EGeneral:Invalid arguments"],"result":{}}"#;
        assert!(matches!(
            parse_spot(body),
            Err(SourceError::Rejected { reason }) if reason.contains("EGeneral")
        ));
    }

    #[test]
    fn futures_split_perpetuals_from_dated_contracts() {
        let instruments = parse_futures(FUTURES).unwrap();
        assert!(!instruments.is_empty());

        let perp = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Perpetual)
            .expect("the fixture carries a perpetual");
        assert_eq!(perp.contract.as_ref().unwrap().expiry, None);

        if let Some(dated) = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Future)
        {
            assert!(
                dated.contract.as_ref().unwrap().expiry.is_some(),
                "a dated contract must carry its last trading time"
            );
        }
    }

    #[test]
    fn inverse_futures_settle_in_the_base_coin() {
        let instruments = parse_futures(FUTURES).unwrap();
        if let Some(inverse) = instruments
            .iter()
            .find(|i| i.contract.as_ref().unwrap().settlement == Settlement::Inverse)
        {
            assert_eq!(inverse.contract.as_ref().unwrap().settle, inverse.base);
        }
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
        assert!(parse_futures(b"<html>nope</html>").is_err());
    }

    #[test]
    fn maps_documented_statuses() {
        assert_eq!(map_spot_status("online", "X"), InstrumentStatus::Trading);
        assert_eq!(
            map_spot_status("cancel_only", "X"),
            InstrumentStatus::Halted
        );
        assert_eq!(map_spot_status("weird", "X"), InstrumentStatus::Unknown);
    }
}
