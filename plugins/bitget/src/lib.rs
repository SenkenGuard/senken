//! Bitget market data for Senken: spot pairs and the three futures
//! product lines.
//!
//! Bitget splits futures by margin currency in the query rather than by a
//! field, so `USDT-FUTURES`, `USDC-FUTURES` and `COIN-FUTURES` are three
//! sources — and which of them is linear or inverse follows from that
//! choice, not from anything in the payload.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::decimal::{format_scaled, parse_increment};
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{Envelope, RawContract, RawSymbol};

mod api;

/// Source id of the spot market.
pub const SPOT_ID: &str = "bitget-spot";
/// Source id of the USDT-margined futures market.
pub const USDT_ID: &str = "bitget-usdt-futures";
/// Source id of the USDC-margined futures market.
pub const USDC_ID: &str = "bitget-usdc-futures";
/// Source id of the coin-margined futures market.
pub const COIN_ID: &str = "bitget-coin-futures";

const SPOT_URL: &str = "https://api.bitget.com/api/v2/spot/public/symbols";
const MIX_URL: &str = "https://api.bitget.com/api/v2/mix/market/contracts";
/// Bitget's success code, as a string.
const OK: &str = "00000";

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "Bitget Spot", SPOT_URL, client, parse_spot)
}

/// USDT-margined futures: linear perpetuals and delivery contracts.
#[must_use]
pub fn usdt_futures_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        USDT_ID,
        "Bitget USDT Futures",
        format!("{MIX_URL}?productType=USDT-FUTURES"),
        client,
        parse_linear,
    )
}

/// USDC-margined futures: linear perpetuals and delivery contracts.
#[must_use]
pub fn usdc_futures_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        USDC_ID,
        "Bitget USDC Futures",
        format!("{MIX_URL}?productType=USDC-FUTURES"),
        client,
        parse_linear,
    )
}

/// Coin-margined futures: inverse perpetuals and delivery contracts.
#[must_use]
pub fn coin_futures_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        COIN_ID,
        "Bitget COIN Futures",
        format!("{MIX_URL}?productType=COIN-FUTURES"),
        client,
        parse_inverse,
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
    if raw.base_coin.is_empty() || raw.quote_coin.is_empty() {
        return skip(SPOT_ID, &raw.symbol, "missing base or quote coin");
    }
    // Bitget spot has no decimal tick field at all.
    let Some(price) = raw.price_precision.precision() else {
        return skip(SPOT_ID, &raw.symbol, "unusable pricePrecision");
    };
    let Some(qty) = raw.quantity_precision.precision() else {
        return skip(SPOT_ID, &raw.symbol, "unusable quantityPrecision");
    };

    let status = match raw.status.as_str() {
        "online" => InstrumentStatus::Trading,
        "halt" | "gray" => InstrumentStatus::Halted,
        "offline" => InstrumentStatus::Closed,
        _ => InstrumentStatus::Unknown,
    };

    Some(
        Instrument::spot(
            normalise_symbol(&raw.symbol, &[]),
            raw.symbol,
            raw.base_coin,
            raw.quote_coin,
        )
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn parse_linear(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse_mix(body, Settlement::Linear)
}

fn parse_inverse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse_mix(body, Settlement::Inverse)
}

fn parse_mix(body: &[u8], settlement: Settlement) -> Result<Vec<Instrument>, SourceError> {
    Ok(decode::<RawContract>(body)?
        .into_iter()
        .filter_map(|raw| mix_instrument(raw, settlement))
        .collect())
}

fn mix_instrument(raw: RawContract, settlement: Settlement) -> Option<Instrument> {
    let source = if settlement == Settlement::Inverse {
        COIN_ID
    } else {
        USDT_ID
    };
    if raw.base_coin.is_empty() || raw.quote_coin.is_empty() {
        return skip(source, &raw.symbol, "missing base or quote coin");
    }
    let Some(price) = mix_price_increment(&raw) else {
        return skip(source, &raw.symbol, "unusable pricePlace/priceEndStep");
    };
    // `sizeMultiplier` and `volumePlace` disagree on some symbols; the
    // multiplier is the one that matches real order rejections.
    let Some(qty) = raw.size_multiplier.increment() else {
        return skip(source, &raw.symbol, "unusable sizeMultiplier");
    };

    let settle = raw.support_margin_coins.first().map_or_else(
        || {
            if settlement == Settlement::Inverse {
                raw.base_coin.clone()
            } else {
                raw.quote_coin.clone()
            }
        },
        Clone::clone,
    );

    let expiry = raw.delivery_time.as_i64().filter(|ms| *ms > 0);
    let kind = if raw.symbol_type.eq_ignore_ascii_case("delivery") || expiry.is_some() {
        InstrumentKind::Future
    } else {
        InstrumentKind::Perpetual
    };

    let mut contract = Contract::new(settle, settlement);
    if let Some(expiry) = expiry {
        let Some(expiry) = UnixNanos::from_millis(expiry) else {
            return skip(source, &raw.symbol, "deliveryTime overflowed UnixNanos");
        };
        contract = contract.with_expiry(expiry);
    }

    let status = match raw.symbol_status.as_str() {
        "normal" => InstrumentStatus::Trading,
        "maintain" | "limit_open" | "restrictedAPI" => InstrumentStatus::Halted,
        "off" => InstrumentStatus::Closed,
        _ => InstrumentStatus::Unknown,
    };

    let name = match kind {
        InstrumentKind::Perpetual => format!("{} / {} perpetual", raw.base_coin, raw.quote_coin),
        _ => format!("{} / {} future", raw.base_coin, raw.quote_coin),
    };

    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &[]),
            raw.symbol,
            raw.base_coin,
            raw.quote_coin,
            kind,
            contract,
        )
        .with_name(name)
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

/// The futures price tick is `priceEndStep` counted in units of the last
/// decimal place, so `priceEndStep = 1` at `pricePlace = 1` is a tick of
/// `0.1` — not `1`.
fn mix_price_increment(raw: &RawContract) -> Option<(u8, i64)> {
    let (scale, _) = raw.price_place.precision()?;
    let steps = raw.price_end_step.as_i64().filter(|steps| *steps > 0)?;
    // Re-read the assembled tick so the pair keeps the crate's minimal-scale
    // contract: `10` steps at scale 2 is a tick of `0.1`, i.e. `(1, 1)`.
    parse_increment(&format_scaled(steps, scale))
}

/// Registers every Bitget market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct BitgetPlugin;

impl Plugin for BitgetPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "bitget".to_owned(),
            name: "Bitget".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Bitget spot and USDT/USDC/COIN futures market data".to_owned(),
            permissions: Vec::new(),
        }
    }

    fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        let group = context.limit_group("bitget");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(usdt_futures_source(client.clone())));
        context.register_marketdata_source(Arc::new(usdc_futures_source(client.clone())));
        context.register_marketdata_source(Arc::new(coin_futures_source(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_inverse, parse_linear, parse_spot};
    use senken_marketdata::instrument::{InstrumentKind, Settlement};
    use senken_marketdata::source::SourceError;

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const USDT: &[u8] = include_bytes!("../tests/fixtures/usdt_futures.json");
    const COIN: &[u8] = include_bytes!("../tests/fixtures/coin_futures.json");

    #[test]
    fn spot_precision_counts_become_increments() {
        let instruments = parse_spot(SPOT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!(btc.kind, InstrumentKind::Spot);
        assert_eq!((btc.price_scale, btc.tick_size), (2, 1));
        assert_eq!((btc.qty_scale, btc.step_size), (6, 1));
    }

    #[test]
    fn the_futures_tick_is_the_end_step_scaled_by_the_price_place() {
        // BTCUSDT: priceEndStep 1 at pricePlace 1 is a tick of 0.1.
        let instruments = parse_linear(USDT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();
        assert_eq!(
            (btc.price_scale, btc.tick_size),
            (1, 1),
            "reading priceEndStep as the tick would give 1, not 0.1"
        );
    }

    #[test]
    fn the_quantity_step_comes_from_the_size_multiplier() {
        let instruments = parse_linear(USDT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();
        assert_eq!((btc.qty_scale, btc.step_size), (4, 1));
    }

    #[test]
    fn the_product_line_decides_linear_or_inverse() {
        let linear = parse_linear(USDT).unwrap();
        assert!(
            linear
                .iter()
                .all(|i| i.contract.as_ref().unwrap().settlement == Settlement::Linear)
        );

        let inverse = parse_inverse(COIN).unwrap();
        assert!(
            inverse
                .iter()
                .all(|i| i.contract.as_ref().unwrap().settlement == Settlement::Inverse)
        );
    }

    #[test]
    fn a_delivery_contract_carries_its_expiry() {
        let instruments = parse_inverse(COIN).unwrap();
        if let Some(dated) = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Future)
        {
            assert!(dated.contract.as_ref().unwrap().expiry.is_some());
        }
    }

    #[test]
    fn a_failure_code_is_a_rejection() {
        let body = br#"{"code":"40034","msg":"Parameter does not exist","data":[]}"#;
        assert!(matches!(
            parse_spot(body),
            Err(SourceError::Rejected { reason }) if reason.contains("40034")
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
    }
}
