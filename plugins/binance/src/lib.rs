//! Binance market data for Senken: spot, USDⓈ-margined futures and
//! coin-margined futures.
//!
//! Each market is its own [`MarketDataSource`], so `binance-spot`,
//! `binance-usdm` and `binance-coinm` are searched, cached and refreshed
//! independently. [`BinancePlugin`] registers all three; the source
//! constructors are public, so a library user can take just the one market
//! they care about without any plugin machinery.
//!
//! # No live feed from this network
//!
//! `wss://stream.binance.com:9443/ws` could not be reached at all from the
//! machine this adapter was written on — the socket never opened, the same
//! restriction that keeps this plugin's bar source unwritten. Nothing
//! about the feed is claimed either way; it is simply unrecorded, and a
//! protocol written from memory of the documentation is what this
//! project's fixtures exist to prevent.
//!
//! [`MarketDataSource`]: senken_marketdata::MarketDataSource

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{ExchangeInfo, RawSymbol};

mod api;
mod bars;

pub use crate::bars::{BinanceBarSource, bar_source as bar_source_spot};

/// Source id of the spot market.
pub const SPOT_ID: &str = "binance-spot";
/// Source id of the USDⓈ-margined futures market.
pub const USDM_ID: &str = "binance-usdm";
/// Source id of the coin-margined futures market.
pub const COINM_ID: &str = "binance-coinm";

const SPOT_URL: &str = "https://api.binance.com/api/v3/exchangeInfo?permissions=SPOT";
const USDM_URL: &str = "https://fapi.binance.com/fapi/v1/exchangeInfo";
const COINM_URL: &str = "https://dapi.binance.com/dapi/v1/exchangeInfo";

/// Which Binance market a document came from. The three endpoints share a
/// shape but not their meaning: the same symbol is a spot pair on one and a
/// linear perpetual on another.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Market {
    Spot,
    UsdM,
    CoinM,
}

impl Market {
    fn source_id(self) -> &'static str {
        match self {
            Self::Spot => SPOT_ID,
            Self::UsdM => USDM_ID,
            Self::CoinM => COINM_ID,
        }
    }

    /// How a contract on this market settles. Spot settles nothing.
    fn settlement(self) -> Option<Settlement> {
        match self {
            Self::Spot => None,
            Self::UsdM => Some(Settlement::Linear),
            Self::CoinM => Some(Settlement::Inverse),
        }
    }
}

/// The spot market: every pair on `api.binance.com`.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "Binance Spot", SPOT_URL, client, parse_spot)
}

/// The USDⓈ-margined futures market: linear perpetuals and quarterlies.
#[must_use]
pub fn usdm_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        USDM_ID,
        "Binance USDⓈ-M Futures",
        USDM_URL,
        client,
        parse_usdm,
    )
}

/// The coin-margined futures market: inverse perpetuals and quarterlies.
#[must_use]
pub fn coinm_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        COINM_ID,
        "Binance COIN-M Futures",
        COINM_URL,
        client,
        parse_coinm,
    )
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::Spot)
}

fn parse_usdm(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::UsdM)
}

fn parse_coinm(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::CoinM)
}

/// Decodes an `exchangeInfo` document, skipping (and logging) any symbol
/// that cannot satisfy the fixed-point contract rather than failing the
/// whole catalog for one bad row.
fn parse(body: &[u8], market: Market) -> Result<Vec<Instrument>, SourceError> {
    let info: ExchangeInfo = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(info
        .symbols
        .into_iter()
        .filter_map(|symbol| to_instrument(symbol, market))
        .collect())
}

fn to_instrument(raw: RawSymbol, market: Market) -> Option<Instrument> {
    let source = market.source_id();
    if raw.symbol.trim().is_empty() {
        return skip(source, "", "empty symbol");
    }
    if raw.base_asset.is_empty() || raw.quote_asset.is_empty() {
        return skip(source, &raw.symbol, "missing base or quote asset");
    }

    let Some(price) = raw.tick().and_then(senken_venue::Num::increment) else {
        return skip(source, &raw.symbol, "PRICE_FILTER missing or unusable");
    };
    let Some(qty) = raw.step().and_then(senken_venue::Num::increment) else {
        return skip(source, &raw.symbol, "LOT_SIZE missing or unusable");
    };

    let status = map_status(raw.state(), &raw.symbol);
    let symbol = normalise_symbol(&raw.symbol, &['_']);

    let instrument = match market.settlement() {
        None => Instrument::spot(symbol, raw.symbol, raw.base_asset, &raw.quote_asset),
        Some(settlement) => {
            let kind = contract_kind(&raw.contract_type);
            let mut contract = Contract::new(settle_of(&raw, market), settlement);
            // A perpetual reports a year-2100 sentinel rather than no
            // delivery date; only a dated contract has a real expiry.
            if kind == InstrumentKind::Future
                && let Some(delivery) = raw.delivery_date.filter(|d| *d > 0)
            {
                let Some(delivery) = UnixNanos::from_millis(delivery) else {
                    return skip(source, &raw.symbol, "deliveryDate overflowed UnixNanos");
                };
                contract = contract.with_expiry(delivery);
            }
            if let Some((scale, size)) = raw
                .contract_size
                .as_ref()
                .and_then(senken_venue::Num::increment)
            {
                contract = contract.with_contract_size(scale, size);
            }
            let name = derivative_name(&raw, kind);
            Instrument::derivative(
                symbol,
                raw.symbol,
                raw.base_asset,
                raw.quote_asset,
                kind,
                contract,
            )
            .with_name(name)
        }
    };

    Some(
        instrument
            .with_status(status)
            .with_price_increment(price)
            .with_qty_increment(qty),
    )
}

/// What a contract settles in. Binance names it `marginAsset`; fall back to
/// the leg the market is margined in when the field is absent.
fn settle_of(raw: &RawSymbol, market: Market) -> String {
    if !raw.margin_asset.is_empty() {
        return raw.margin_asset.clone();
    }
    match market {
        Market::CoinM => raw.base_asset.clone(),
        Market::Spot | Market::UsdM => raw.quote_asset.clone(),
    }
}

fn contract_kind(contract_type: &str) -> InstrumentKind {
    if contract_type.eq_ignore_ascii_case("PERPETUAL") {
        InstrumentKind::Perpetual
    } else {
        InstrumentKind::Future
    }
}

fn derivative_name(raw: &RawSymbol, kind: InstrumentKind) -> String {
    let (base, quote) = (&raw.base_asset, &raw.quote_asset);
    match kind {
        InstrumentKind::Perpetual => format!("{base} / {quote} perpetual"),
        _ => format!("{base} / {quote} future"),
    }
}

fn map_status(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "TRADING" => InstrumentStatus::Trading,
        "PRE_TRADING" | "PENDING_TRADING" => InstrumentStatus::PreOpen,
        "BREAK" | "HALT" | "AUCTION_MATCH" => InstrumentStatus::Halted,
        "END_OF_DAY" | "POST_TRADING" | "CLOSE" | "PRE_DELIVERING" | "DELIVERING" | "DELIVERED"
        | "PRE_SETTLE" | "SETTLING" => InstrumentStatus::Closed,
        other => {
            tracing::warn!(symbol, status = other, "unknown binance symbol status");
            InstrumentStatus::Unknown
        }
    }
}

/// Registers every Binance market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct BinancePlugin;

impl Plugin for BinancePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "binance".to_owned(),
            name: "Binance".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Binance spot, USDⓈ-M and COIN-M market data".to_owned(),
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
        // One shared client, three markets: which markets a venue exposes
        // is the plugin's decision, not the registry's. Bar traffic shares
        // this same group, so instrument and kline
        // fetches together stay under Binance's one IP-level quota rather
        // than doubling it.
        let group = context.limit_group("binance");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(usdm_source(client.clone())));
        context.register_marketdata_source(Arc::new(coinm_source(client.clone())));
        context.register_bar_source(Arc::new(bar_source_spot(
            client,
            Arc::new(senken_plugin::SystemClock),
        )));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Market, map_status, parse};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/exchange_info.json");
    const USDM: &[u8] = include_bytes!("../tests/fixtures/usdm_exchange_info.json");
    const COINM: &[u8] = include_bytes!("../tests/fixtures/coinm_exchange_info.json");

    #[test]
    fn spot_normalises_to_the_fixed_point_contract() {
        let instruments = parse(SPOT, Market::Spot).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!(btc.source_symbol, "BTCUSDT");
        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USDT"));
        assert_eq!(btc.status, InstrumentStatus::Trading);
        assert_eq!(btc.kind, InstrumentKind::Spot);
        assert!(btc.contract.is_none(), "spot settles nothing");
        // tick 0.01 → scale 2, size 1 — regardless of quotePrecision=8
        assert_eq!((btc.price_scale, btc.tick_size), (2, 1));
        assert_eq!((btc.qty_scale, btc.step_size), (5, 1));
    }

    #[test]
    fn symbols_without_a_lot_size_are_skipped_not_fatal() {
        let instruments = parse(SPOT, Market::Spot).unwrap();
        assert_eq!(instruments.len(), 3);
        assert!(instruments.iter().all(|i| i.symbol != "BROKENUSDT"));
    }

    #[test]
    fn usdm_perpetuals_are_linear_and_never_expire() {
        let instruments = parse(USDM, Market::UsdM).unwrap();
        let perp = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!(perp.kind, InstrumentKind::Perpetual);
        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.settle, "USDT");
        assert_eq!(
            contract.expiry, None,
            "the year-2100 sentinel is not a real expiry"
        );
        assert_eq!((perp.price_scale, perp.tick_size), (1, 1));
    }

    #[test]
    fn usdm_quarterlies_keep_their_expiry_and_their_own_symbol() {
        let instruments = parse(USDM, Market::UsdM).unwrap();
        let dated = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Future)
            .unwrap();

        assert!(
            dated.symbol.starts_with("BTCUSDT") && dated.symbol != "BTCUSDT",
            "a dated future must not collapse onto the perpetual: {}",
            dated.symbol
        );
        assert!(dated.contract.as_ref().unwrap().expiry.is_some());
    }

    #[test]
    fn coinm_contracts_are_inverse_and_settle_in_the_base_coin() {
        let instruments = parse(COINM, Market::CoinM).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.symbol == "BTCUSDPERP")
            .unwrap();

        assert_eq!(perp.kind, InstrumentKind::Perpetual);
        assert_eq!(perp.source_symbol, "BTCUSD_PERP");
        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Inverse);
        assert_eq!(contract.settle, "BTC");
        // 100 USD of notional per contract
        assert_eq!((contract.size_scale, contract.contract_size), (0, 100));
    }

    #[test]
    fn coinm_reports_its_state_under_a_different_field() {
        // dapi says `contractStatus`, everyone else says `status`.
        let instruments = parse(COINM, Market::CoinM).unwrap();
        assert!(
            instruments
                .iter()
                .any(|i| i.status == InstrumentStatus::Trading),
            "contractStatus must be read, or every contract looks Unknown"
        );
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse(b"<html>rate limited</html>", Market::Spot).is_err());
    }

    #[test]
    fn maps_every_documented_status() {
        assert_eq!(map_status("TRADING", "X"), InstrumentStatus::Trading);
        assert_eq!(map_status("PRE_TRADING", "X"), InstrumentStatus::PreOpen);
        assert_eq!(
            map_status("PENDING_TRADING", "X"),
            InstrumentStatus::PreOpen
        );
        assert_eq!(map_status("BREAK", "X"), InstrumentStatus::Halted);
        assert_eq!(map_status("DELIVERING", "X"), InstrumentStatus::Closed);
        assert_eq!(map_status("SOMETHING_NEW", "X"), InstrumentStatus::Unknown);
    }
}
