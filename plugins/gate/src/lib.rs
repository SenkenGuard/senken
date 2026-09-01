//! Gate.io market data for Senken: spot, USDT and BTC perpetuals, and USDT
//! delivery futures.
//!
//! Gate splits its derivatives by settlement currency in the URL rather
//! than by a field, so each is its own source. Two traps it handles: the
//! `type` field is what says linear or inverse (`contract_type` is an
//! asset-class tag), and delivery expiries are in **seconds**, not
//! milliseconds.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{RawContract, RawPair};

mod api;

/// Source id of the spot market.
pub const SPOT_ID: &str = "gate-spot";
/// Source id of the USDT-settled perpetual market.
pub const USDT_PERP_ID: &str = "gate-usdt-perp";
/// Source id of the BTC-settled perpetual market.
pub const BTC_PERP_ID: &str = "gate-btc-perp";
/// Source id of the USDT-settled delivery market.
pub const USDT_DELIVERY_ID: &str = "gate-usdt-delivery";

const BASE_URL: &str = "https://api.gateio.ws/api/v4";

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        SPOT_ID,
        "Gate.io Spot",
        format!("{BASE_URL}/spot/currency_pairs"),
        client,
        parse_spot,
    )
}

/// USDT-settled perpetuals.
#[must_use]
pub fn usdt_perp_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        USDT_PERP_ID,
        "Gate.io USDT Perpetuals",
        format!("{BASE_URL}/futures/usdt/contracts"),
        client,
        parse_perp,
    )
}

/// BTC-settled perpetuals.
#[must_use]
pub fn btc_perp_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        BTC_PERP_ID,
        "Gate.io BTC Perpetuals",
        format!("{BASE_URL}/futures/btc/contracts"),
        client,
        parse_perp,
    )
}

/// USDT-settled dated delivery futures.
#[must_use]
pub fn usdt_delivery_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        USDT_DELIVERY_ID,
        "Gate.io USDT Delivery",
        format!("{BASE_URL}/delivery/usdt/contracts"),
        client,
        parse_delivery,
    )
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let pairs: Vec<RawPair> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(pairs.into_iter().filter_map(spot_instrument).collect())
}

fn spot_instrument(raw: RawPair) -> Option<Instrument> {
    if raw.base.is_empty() || raw.quote.is_empty() {
        return skip(SPOT_ID, &raw.id, "missing base or quote");
    }
    // Spot publishes only decimal-place counts, never an increment.
    let Some(price) = raw.precision.precision() else {
        return skip(SPOT_ID, &raw.id, "unusable precision");
    };
    let Some(qty) = raw.amount_precision.precision() else {
        return skip(SPOT_ID, &raw.id, "unusable amount_precision");
    };

    let status = match raw.trade_status.as_str() {
        "tradable" => InstrumentStatus::Trading,
        "untradable" => InstrumentStatus::Halted,
        _ => InstrumentStatus::Unknown,
    };

    Some(
        Instrument::spot(
            normalise_symbol(&raw.id, &['_']),
            raw.id,
            raw.base,
            raw.quote,
        )
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn parse_perp(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse_contracts(body, false)
}

fn parse_delivery(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse_contracts(body, true)
}

fn parse_contracts(body: &[u8], dated: bool) -> Result<Vec<Instrument>, SourceError> {
    let contracts: Vec<RawContract> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(contracts
        .into_iter()
        .filter_map(|raw| contract_instrument(raw, dated))
        .collect())
}

fn contract_instrument(raw: RawContract, dated: bool) -> Option<Instrument> {
    let source = if dated {
        USDT_DELIVERY_ID
    } else {
        USDT_PERP_ID
    };
    // Gate gives no base/quote fields on derivatives; the name carries both.
    let Some((base, quote)) = raw.name.split_once('_') else {
        return skip(source, &raw.name, "contract name has no separator");
    };
    let (base, quote) = (base.to_owned(), quote.to_owned());
    let Some(price) = raw.order_price_round.increment() else {
        return skip(source, &raw.name, "unusable order_price_round");
    };
    // Gate quotes derivative quantities in whole contracts, so the step is
    // always one. `order_size_min` is a *minimum* — and it is `0` on
    // ETH_USDT, SOL_USDT and other majors, which would drop them entirely
    // if it were read as the step.
    let qty = (0, 1);

    let settlement = if raw.contract_kind == "inverse" {
        Settlement::Inverse
    } else {
        Settlement::Linear
    };
    let settle = if settlement == Settlement::Inverse {
        base.clone()
    } else {
        quote.clone()
    };

    // Gate reports delivery expiry in seconds; everything else here is ms.
    // `UnixNanos::from_secs` names that unit explicitly — this is the exact
    // trap `UnixNanos` exists to make unrepresentable, so it must never
    // become `from_millis`.
    let expiry = raw.expire_time.as_i64().filter(|seconds| *seconds > 0);
    let kind = if expiry.is_some() {
        InstrumentKind::Future
    } else {
        InstrumentKind::Perpetual
    };

    let mut contract = Contract::new(settle, settlement);
    if let Some(seconds) = expiry {
        let Some(expiry) = UnixNanos::from_secs(seconds) else {
            return skip(source, &raw.name, "expire_time overflowed UnixNanos");
        };
        contract = contract.with_expiry(expiry);
    }
    if let Some((scale, size)) = raw.quanto_multiplier.increment() {
        contract = contract.with_contract_size(scale, size);
    }

    let status = if raw.in_delisting {
        InstrumentStatus::Closed
    } else if raw.status == "trading" {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Halted
    };

    let name = match kind {
        InstrumentKind::Perpetual => format!("{base} / {quote} perpetual"),
        _ => format!("{base} / {quote} future"),
    };

    Some(
        Instrument::derivative(
            normalise_symbol(&raw.name, &['_']),
            raw.name,
            base,
            quote,
            kind,
            contract,
        )
        .with_name(name)
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

/// Registers every Gate.io market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct GatePlugin;

impl Plugin for GatePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "gate".to_owned(),
            name: "Gate.io".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Gate.io spot, perpetual and delivery market data".to_owned(),
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
        let group = context.limit_group("gate");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(usdt_perp_source(client.clone())));
        context.register_marketdata_source(Arc::new(btc_perp_source(client.clone())));
        context.register_marketdata_source(Arc::new(usdt_delivery_source(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_delivery, parse_perp, parse_spot};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const PERP: &[u8] = include_bytes!("../tests/fixtures/usdt_perp.json");
    const DELIVERY: &[u8] = include_bytes!("../tests/fixtures/usdt_delivery.json");

    #[test]
    fn spot_precision_counts_become_increments() {
        let instruments = parse_spot(SPOT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!(btc.source_symbol, "BTC_USDT");
        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USDT"));
        assert_eq!(btc.kind, InstrumentKind::Spot);
        // A precision of N means a tick of 1 at scale N.
        assert_eq!(btc.tick_size, 1);
        assert_eq!(btc.step_size, 1);
    }

    #[test]
    fn a_perpetual_splits_its_pair_out_of_the_contract_name() {
        let instruments = parse_perp(PERP).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!(btc.base, "BTC");
        assert_eq!(btc.quote, "USDT");
        assert_eq!(btc.kind, InstrumentKind::Perpetual);
        let contract = btc.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.expiry, None);
    }

    #[test]
    fn delivery_expiries_are_seconds_and_become_unix_nanos() {
        let instruments = parse_delivery(DELIVERY).unwrap();
        let dated = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Future)
            .expect("the fixture carries a delivery contract");

        let expiry = dated.contract.as_ref().unwrap().expiry.unwrap();
        // A `seconds * 1_000` regression (the historical Gate bug this
        // guards against) would land three orders of magnitude too early.
        assert!(
            expiry.as_millis() > 1_000_000_000_000,
            "seconds must be scaled via UnixNanos::from_secs, got {expiry}"
        );
    }

    #[test]
    fn a_delisting_contract_is_closed() {
        let instruments = parse_perp(PERP).unwrap();
        assert!(
            instruments
                .iter()
                .all(|i| i.status != InstrumentStatus::Unknown)
        );
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
        assert!(parse_perp(b"<html>nope</html>").is_err());
    }
}
