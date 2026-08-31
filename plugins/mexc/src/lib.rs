//! MEXC market data for Senken: spot pairs and futures contracts.
//!
//! Both markets are served from `api.mexc.com`; the dedicated
//! `contract.mexc.com` host answers 403 to plain clients, so the futures
//! path is taken from the main host instead.

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{ContractDetail, ExchangeInfo, RawContract, RawSymbol};

mod api;

/// Source id of the spot market.
pub const SPOT_ID: &str = "mexc-spot";
/// Source id of the futures market.
pub const FUTURES_ID: &str = "mexc-futures";

const SPOT_URL: &str = "https://api.mexc.com/api/v3/exchangeInfo";
const FUTURES_URL: &str = "https://api.mexc.com/api/v1/contract/detail";

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "MEXC Spot", SPOT_URL, client, parse_spot)
}

/// The futures market: perpetuals, linear and inverse.
#[must_use]
pub fn futures_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        FUTURES_ID,
        "MEXC Futures",
        FUTURES_URL,
        client,
        parse_futures,
    )
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let info: ExchangeInfo = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(info
        .symbols
        .into_iter()
        .filter_map(spot_instrument)
        .collect())
}

fn spot_instrument(raw: RawSymbol) -> Option<Instrument> {
    if raw.base_asset.is_empty() || raw.quote_asset.is_empty() {
        return skip(SPOT_ID, &raw.symbol, "missing base or quote asset");
    }
    // MEXC ships no price or lot filter, so the precision fields are the
    // only description of the increments there is.
    let Some(price) = raw.quote_precision.precision() else {
        return skip(SPOT_ID, &raw.symbol, "unusable quotePrecision");
    };
    let Some(qty) = raw.base_asset_precision.precision() else {
        return skip(SPOT_ID, &raw.symbol, "unusable baseAssetPrecision");
    };

    // `status` is the string "1" on every symbol, so it says nothing.
    let status = if raw.st {
        InstrumentStatus::Halted
    } else if raw.is_spot_trading_allowed {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Closed
    };

    Some(
        Instrument::spot(
            normalise_symbol(&raw.symbol, &['_']),
            raw.symbol,
            raw.base_asset,
            raw.quote_asset,
        )
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn parse_futures(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let detail: ContractDetail = serde_json::from_slice(body).map_err(SourceError::decode)?;
    if !detail.success {
        return Err(SourceError::rejected(format!("code {}", detail.code)));
    }
    Ok(detail
        .data
        .into_iter()
        .filter_map(futures_instrument)
        .collect())
}

fn futures_instrument(raw: RawContract) -> Option<Instrument> {
    if raw.base_coin.is_empty() || raw.quote_coin.is_empty() {
        return skip(FUTURES_ID, &raw.symbol, "missing base or quote coin");
    }
    let Some(price) = raw.price_unit.increment() else {
        return skip(FUTURES_ID, &raw.symbol, "unusable priceUnit");
    };
    let Some(qty) = raw.vol_unit.increment() else {
        return skip(FUTURES_ID, &raw.symbol, "unusable volUnit");
    };

    // `futureType` is 1 for every contract, so what a contract settles in
    // is the only thing that separates linear from inverse.
    let settle = if raw.settle_coin.is_empty() {
        raw.quote_coin.as_str()
    } else {
        raw.settle_coin.as_str()
    };
    let settlement = if settle.eq_ignore_ascii_case(&raw.base_coin) {
        Settlement::Inverse
    } else {
        Settlement::Linear
    };

    let mut contract = Contract::new(settle, settlement);
    if let Some((scale, size)) = raw.contract_size.increment() {
        contract = contract.with_contract_size(scale, size);
    }

    let status = if raw.state == 0 {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Halted
    };
    let name = format!("{} / {} perpetual", raw.base_coin, raw.quote_coin);

    Some(
        Instrument::derivative(
            normalise_symbol(&raw.symbol, &['_']),
            raw.symbol,
            raw.base_coin,
            raw.quote_coin,
            InstrumentKind::Perpetual,
            contract,
        )
        .with_name(name)
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

/// Registers both MEXC markets with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct MexcPlugin;

impl Plugin for MexcPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "mexc".to_owned(),
            name: "MEXC".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "MEXC spot and futures market data".to_owned(),
            permissions: Vec::new(),
        }
    }

    fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        let group = context.limit_group("mexc");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(futures_source(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_futures, parse_spot};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};
    use senken_marketdata::source::SourceError;

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const FUTURES: &[u8] = include_bytes!("../tests/fixtures/futures.json");

    #[test]
    fn spot_increments_come_from_precision_since_there_are_no_filters() {
        let instruments = parse_spot(SPOT).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!(btc.kind, InstrumentKind::Spot);
        // quotePrecision 2 → a tick of 1 at scale 2.
        assert_eq!((btc.price_scale, btc.tick_size), (2, 1));
        assert_eq!((btc.qty_scale, btc.step_size), (8, 1));
    }

    #[test]
    fn trading_permission_decides_the_status_not_the_status_field() {
        // `status` is "1" for every symbol MEXC lists.
        let instruments = parse_spot(SPOT).unwrap();
        assert!(
            instruments
                .iter()
                .any(|i| i.status == InstrumentStatus::Trading)
        );
    }

    #[test]
    fn linear_and_inverse_are_told_apart_by_the_settle_coin() {
        let instruments = parse_futures(FUTURES).unwrap();
        let linear = instruments
            .iter()
            .find(|i| i.contract.as_ref().unwrap().settlement == Settlement::Linear)
            .expect("the fixture carries a linear contract");
        assert_eq!(linear.contract.as_ref().unwrap().settle, linear.quote);

        if let Some(inverse) = instruments
            .iter()
            .find(|i| i.contract.as_ref().unwrap().settlement == Settlement::Inverse)
        {
            assert_eq!(inverse.contract.as_ref().unwrap().settle, inverse.base);
        }
    }

    #[test]
    fn an_unsuccessful_document_is_a_rejection() {
        let body = br#"{"success":false,"code":510,"data":[]}"#;
        assert!(matches!(
            parse_futures(body),
            Err(SourceError::Rejected { .. })
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
        assert!(parse_futures(b"<html>nope</html>").is_err());
    }
}
