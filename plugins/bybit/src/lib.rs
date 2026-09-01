//! Bybit market data for Senken: spot, linear derivatives and inverse
//! derivatives.
//!
//! Bybit's `linear` and `inverse` categories each mix perpetuals with dated
//! futures; the `contractType` field tells them apart, so one source per
//! category yields both kinds. Options are paged — there are thousands —
//! and every source here follows Bybit's cursor to the end.
//!
//! [`MarketDataSource`]: senken_marketdata::MarketDataSource

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, OptionRight, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{InstrumentsResponse, RawInstrument};

mod api;
mod bars;

pub use crate::bars::{BybitBarSource, bar_source as bar_source_spot};

/// Source id of the spot market.
pub const SPOT_ID: &str = "bybit-spot";
/// Source id of the linear (USDT/USDC-settled) derivatives market.
pub const LINEAR_ID: &str = "bybit-linear";
/// Source id of the inverse (coin-settled) derivatives market.
pub const INVERSE_ID: &str = "bybit-inverse";
/// Source id of the options market.
pub const OPTION_ID: &str = "bybit-option";

const BASE_URL: &str = "https://api.bybit.com/v5/market/instruments-info";
/// Bybit's maximum page size.
const PAGE_LIMIT: u32 = 1000;
/// Enough pages for every category at that size, with room to grow. Options
/// are the only one that needs more than a couple.
const MAX_PAGES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Market {
    Spot,
    Linear,
    Inverse,
    Option,
}

impl Market {
    fn source_id(self) -> &'static str {
        match self {
            Self::Spot => SPOT_ID,
            Self::Linear => LINEAR_ID,
            Self::Inverse => INVERSE_ID,
            Self::Option => OPTION_ID,
        }
    }

    fn settlement(self) -> Option<Settlement> {
        match self {
            Self::Spot => None,
            // An option's premium is paid in the settle coin, which Bybit
            // reports per instrument; linear is the right default for the
            // USDC and USDT books.
            Self::Linear | Self::Option => Some(Settlement::Linear),
            Self::Inverse => Some(Settlement::Inverse),
        }
    }
}

fn url(category: &str) -> String {
    format!("{BASE_URL}?category={category}&limit={PAGE_LIMIT}")
}

/// Bybit's paging cursor, empty once the catalog is exhausted.
fn next_cursor(body: &[u8]) -> Option<String> {
    let response: InstrumentsResponse = serde_json::from_slice(body).ok()?;
    let cursor = response.result.next_page_cursor;
    (!cursor.is_empty()).then_some(cursor)
}

/// Follows Bybit's cursor to the end of a category.
fn paged(source: HttpSource) -> HttpSource {
    source.paginated(next_cursor, "cursor", MAX_PAGES)
}

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    paged(HttpSource::new(
        SPOT_ID,
        "Bybit Spot",
        url("spot"),
        client,
        parse_spot,
    ))
}

/// The linear market: USDT/USDC perpetuals and dated futures.
#[must_use]
pub fn linear_source(client: VenueClient) -> HttpSource {
    paged(HttpSource::new(
        LINEAR_ID,
        "Bybit Linear",
        url("linear"),
        client,
        parse_linear,
    ))
}

/// The inverse market: coin-settled perpetuals and dated futures.
#[must_use]
pub fn inverse_source(client: VenueClient) -> HttpSource {
    paged(HttpSource::new(
        INVERSE_ID,
        "Bybit Inverse",
        url("inverse"),
        client,
        parse_inverse,
    ))
}

/// The options market. Thousands of strikes, spread over many pages.
#[must_use]
pub fn option_source(client: VenueClient) -> HttpSource {
    paged(HttpSource::new(
        OPTION_ID,
        "Bybit Options",
        url("option"),
        client,
        parse_option,
    ))
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::Spot)
}

fn parse_linear(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::Linear)
}

fn parse_inverse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::Inverse)
}

fn parse_option(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::Option)
}

fn parse(body: &[u8], market: Market) -> Result<Vec<Instrument>, SourceError> {
    let response: InstrumentsResponse =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    if response.ret_code != 0 {
        return Err(SourceError::rejected(format!(
            "retCode {}: {}",
            response.ret_code, response.ret_msg
        )));
    }
    Ok(response
        .result
        .list
        .into_iter()
        .filter_map(|raw| to_instrument(raw, market))
        .collect())
}

fn to_instrument(raw: RawInstrument, market: Market) -> Option<Instrument> {
    let source = market.source_id();
    if raw.symbol.trim().is_empty() {
        return skip(source, "", "empty symbol");
    }
    if raw.base_coin.is_empty() || raw.quote_coin.is_empty() {
        return skip(source, &raw.symbol, "missing base or quote coin");
    }
    let Some(price) = raw.price_filter.tick_size.increment() else {
        return skip(source, &raw.symbol, "unusable tickSize");
    };
    let Some(qty) = raw.lot_size_filter.step().increment() else {
        return skip(source, &raw.symbol, "unusable quantity step");
    };

    let symbol = normalise_symbol(&raw.symbol, &['-']);
    let status = map_status(&raw.status, &raw.symbol);
    let instrument = match market.settlement() {
        None => Instrument::spot(symbol, raw.symbol, raw.base_coin, &raw.quote_coin),
        Some(settlement) => {
            let kind = if market == Market::Option {
                InstrumentKind::Option
            } else {
                contract_kind(&raw.contract_type)
            };
            let settle = if !raw.settle_coin.is_empty() {
                raw.settle_coin.as_str()
            } else if settlement == Settlement::Inverse {
                raw.base_coin.as_str()
            } else {
                raw.quote_coin.as_str()
            };

            let mut contract = Contract::new(settle, settlement);
            // Bybit reports "0" rather than nothing for a perpetual.
            if let Some(expiry) = raw.delivery_time.as_i64().filter(|ms| *ms > 0) {
                let Some(expiry) = UnixNanos::from_millis(expiry) else {
                    return skip(source, &raw.symbol, "deliveryTime overflowed UnixNanos");
                };
                contract = contract.with_expiry(expiry);
            }
            if let Some(right) = option_right(&raw.options_type)
                && let Some((strike_scale, strike)) = option_strike(&raw.symbol)
            {
                contract = contract.with_option(right, strike_scale, strike);
            }

            let name = match kind {
                InstrumentKind::Perpetual => {
                    format!("{} / {} perpetual", raw.base_coin, raw.quote_coin)
                }
                InstrumentKind::Option => {
                    format!("{} / {} option", raw.base_coin, raw.quote_coin)
                }
                _ => format!("{} / {} future", raw.base_coin, raw.quote_coin),
            };
            Instrument::derivative(
                symbol,
                raw.symbol,
                raw.base_coin,
                raw.quote_coin,
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

fn option_right(options_type: &str) -> Option<OptionRight> {
    match options_type {
        "Call" => Some(OptionRight::Call),
        "Put" => Some(OptionRight::Put),
        _ => None,
    }
}

/// The strike, which Bybit publishes only inside the symbol:
/// `BTC-25JUN27-160000-P-USDT` is struck at 160 000.
fn option_strike(symbol: &str) -> Option<(u8, i64)> {
    let strike = symbol.split('-').nth(2)?;
    senken_marketdata::decimal::parse_increment(strike)
}

/// `LinearFutures` and `InverseFutures` are dated; everything else on a
/// derivatives category is a perpetual.
fn contract_kind(contract_type: &str) -> InstrumentKind {
    if contract_type.ends_with("Futures") {
        InstrumentKind::Future
    } else {
        InstrumentKind::Perpetual
    }
}

fn map_status(raw: &str, symbol: &str) -> InstrumentStatus {
    match raw {
        "Trading" => InstrumentStatus::Trading,
        "PreLaunch" => InstrumentStatus::PreOpen,
        "Delivering" | "Settling" => InstrumentStatus::Halted,
        "Closed" => InstrumentStatus::Closed,
        other => {
            tracing::warn!(symbol, status = other, "unknown bybit instrument status");
            InstrumentStatus::Unknown
        }
    }
}

/// Registers every Bybit market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct BybitPlugin;

impl Plugin for BybitPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "bybit".to_owned(),
            name: "Bybit".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Bybit spot, linear and inverse market data".to_owned(),
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
        let group = context.limit_group("bybit");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(linear_source(client.clone())));
        context.register_marketdata_source(Arc::new(inverse_source(client.clone())));
        context.register_marketdata_source(Arc::new(option_source(client.clone())));
        // Bar traffic shares the same group as every market data source
        // above — one shared budget per venue.
        context.register_bar_source(Arc::new(bar_source_spot(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Market, map_status, parse};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};
    use senken_marketdata::source::SourceError;

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const LINEAR: &[u8] = include_bytes!("../tests/fixtures/linear.json");
    const INVERSE: &[u8] = include_bytes!("../tests/fixtures/inverse.json");
    const OPTION: &[u8] = include_bytes!("../tests/fixtures/option.json");

    #[test]
    fn options_carry_a_strike_read_out_of_the_symbol() {
        // Bybit publishes the strike nowhere but the symbol itself.
        let instruments = parse(OPTION, Market::Option).unwrap();
        let call = instruments
            .iter()
            .find(|i| i.source_symbol.contains("-C-"))
            .expect("the fixture carries a call");

        assert_eq!(call.kind, InstrumentKind::Option);
        let terms = call.contract.as_ref().unwrap().option.as_ref().unwrap();
        assert_eq!(terms.right, senken_marketdata::OptionRight::Call);
        assert_eq!(terms.strike, 160_000);
        assert!(call.contract.as_ref().unwrap().expiry.is_some());
    }

    #[test]
    fn a_cursor_is_read_only_while_more_pages_remain() {
        assert_eq!(super::next_cursor(OPTION), None, "the fixture is one page");
        let more = br#"{"retCode":0,"result":{"list":[],"nextPageCursor":"page%3D2"}}"#;
        assert_eq!(super::next_cursor(more).as_deref(), Some("page%3D2"));
    }

    #[test]
    fn spot_reads_its_step_from_base_precision() {
        // Spot names the quantity step `basePrecision`, not `qtyStep`.
        let instruments = parse(SPOT, Market::Spot).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USDT"));
        assert_eq!(btc.kind, InstrumentKind::Spot);
        assert!(btc.contract.is_none());
        assert!(btc.tick_size >= 1 && btc.step_size >= 1);
    }

    #[test]
    fn linear_perpetuals_settle_in_the_quote_currency() {
        let instruments = parse(LINEAR, Market::Linear).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Perpetual)
            .unwrap();

        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.expiry, None);
    }

    #[test]
    fn inverse_contracts_settle_in_the_base_coin() {
        let instruments = parse(INVERSE, Market::Inverse).unwrap();
        let any = instruments.first().unwrap();
        let contract = any.contract.as_ref().unwrap();

        assert_eq!(contract.settlement, Settlement::Inverse);
        assert_eq!(contract.settle, any.base);
    }

    #[test]
    fn a_retcode_is_a_rejection() {
        let body = br#"{"retCode":10001,"retMsg":"Category is invalid","result":{}}"#;
        assert!(matches!(
            parse(body, Market::Spot),
            Err(SourceError::Rejected { reason }) if reason.contains("10001")
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse(b"<html>blocked</html>", Market::Spot).is_err());
    }

    #[test]
    fn maps_every_documented_status() {
        assert_eq!(map_status("Trading", "X"), InstrumentStatus::Trading);
        assert_eq!(map_status("PreLaunch", "X"), InstrumentStatus::PreOpen);
        assert_eq!(map_status("Delivering", "X"), InstrumentStatus::Halted);
        assert_eq!(map_status("Closed", "X"), InstrumentStatus::Closed);
        assert_eq!(map_status("Whatever", "X"), InstrumentStatus::Unknown);
    }
}
