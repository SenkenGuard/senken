//! OKX market data for Senken: spot, perpetual swaps, dated futures and
//! options.
//!
//! Each market is its own [`MarketDataSource`]. Options are listed per
//! underlying family — OKX refuses to enumerate them all at once — so
//! [`option_source`] takes the family and the plugin registers the liquid
//! ones; add more with a call per family.
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

pub use crate::bars::{OkxBarSource, bar_source as bar_source_spot};

/// Source id of the spot market.
pub const SPOT_ID: &str = "okx-spot";
/// Source id of the perpetual swap market.
pub const SWAP_ID: &str = "okx-swap";
/// Source id of the dated futures market.
pub const FUTURES_ID: &str = "okx-futures";

const BASE_URL: &str = "https://www.okx.com/api/v5/public/instruments";

/// Option families the plugin registers by default. OKX lists options one
/// underlying at a time, and these are the two with real liquidity.
const DEFAULT_OPTION_FAMILIES: [&str; 2] = ["BTC-USD", "ETH-USD"];

/// Which OKX market a document came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Market {
    Spot,
    Swap,
    Futures,
    Option,
}

impl Market {
    fn kind(self) -> InstrumentKind {
        match self {
            Self::Spot => InstrumentKind::Spot,
            Self::Swap => InstrumentKind::Perpetual,
            Self::Futures => InstrumentKind::Future,
            Self::Option => InstrumentKind::Option,
        }
    }
}

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        SPOT_ID,
        "OKX Spot",
        format!("{BASE_URL}?instType=SPOT"),
        client,
        parse_spot,
    )
}

/// The perpetual swap market, linear and inverse.
#[must_use]
pub fn swap_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        SWAP_ID,
        "OKX Swap",
        format!("{BASE_URL}?instType=SWAP"),
        client,
        parse_swap,
    )
}

/// The dated futures market.
#[must_use]
pub fn futures_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        FUTURES_ID,
        "OKX Futures",
        format!("{BASE_URL}?instType=FUTURES"),
        client,
        parse_futures,
    )
}

/// The options of one underlying family, such as `BTC-USD`.
///
/// The source id is `okx-option-<family>` in lower case, so each family is
/// searched and cached on its own.
#[must_use]
pub fn option_source(client: VenueClient, family: &str) -> HttpSource {
    HttpSource::new(
        format!("okx-option-{}", family.to_ascii_lowercase()),
        format!("OKX {family} Options"),
        format!("{BASE_URL}?instType=OPTION&instFamily={family}"),
        client,
        parse_option,
    )
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::Spot)
}

fn parse_swap(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::Swap)
}

fn parse_futures(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::Futures)
}

fn parse_option(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, Market::Option)
}

/// Decodes an `instruments` document, skipping (and logging) any entry that
/// cannot satisfy the fixed-point contract.
fn parse(body: &[u8], market: Market) -> Result<Vec<Instrument>, SourceError> {
    let response: InstrumentsResponse =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    if response.code != "0" {
        return Err(SourceError::rejected(format!(
            "code {}: {}",
            response.code, response.msg
        )));
    }
    Ok(response
        .data
        .into_iter()
        .filter_map(|raw| to_instrument(raw, market))
        .collect())
}

fn to_instrument(raw: RawInstrument, market: Market) -> Option<Instrument> {
    let source = "okx";
    if raw.inst_id.trim().is_empty() {
        return skip(source, "", "empty instId");
    }

    let Some((base, quote)) = pair_of(&raw) else {
        return skip(source, &raw.inst_id, "no base/quote and no usable uly");
    };
    let Some(price) = raw.tick_sz.increment() else {
        return skip(source, &raw.inst_id, "unusable tickSz");
    };
    let Some(qty) = raw.lot_sz.increment() else {
        return skip(source, &raw.inst_id, "unusable lotSz");
    };

    let symbol = normalise_symbol(raw.inst_id.trim_end_matches("-SWAP"), &['-']);
    let kind = market.kind();

    let status = map_status(&raw.state, &raw.inst_id);
    let name = name_of(base, quote, kind);
    let (base, quote) = (base.to_owned(), quote.to_owned());

    let instrument = if kind == InstrumentKind::Spot {
        Instrument::spot(symbol, raw.inst_id, base, quote)
    } else {
        let settlement = if raw.ct_type.eq_ignore_ascii_case("inverse") {
            Settlement::Inverse
        } else {
            Settlement::Linear
        };
        let settle = if raw.settle_ccy.is_empty() {
            quote.clone()
        } else {
            raw.settle_ccy.clone()
        };

        let mut contract = Contract::new(settle, settlement);
        if let Some(expiry) = raw.exp_time.as_i64().filter(|ms| *ms > 0) {
            let Some(expiry) = UnixNanos::from_millis(expiry) else {
                return skip(source, &raw.inst_id, "expTime overflowed UnixNanos");
            };
            contract = contract.with_expiry(expiry);
        }
        if let Some((scale, size)) = raw.ct_val.increment() {
            contract = contract.with_contract_size(scale, size);
        }
        if let Some(right) = option_right(&raw.opt_type) {
            let (strike_scale, strike) = raw.stk.increment()?;
            contract = contract.with_option(right, strike_scale, strike);
        }

        Instrument::derivative(symbol, raw.inst_id, base, quote, kind, contract).with_name(name)
    };

    Some(
        instrument
            .with_status(status)
            .with_price_increment(price)
            .with_qty_increment(qty),
    )
}

/// Base and quote for any instrument type.
///
/// Spot carries them directly; every derivative leaves them empty and puts
/// the pair in `uly` instead.
fn pair_of(raw: &RawInstrument) -> Option<(&str, &str)> {
    if !raw.base_ccy.is_empty() && !raw.quote_ccy.is_empty() {
        return Some((&raw.base_ccy, &raw.quote_ccy));
    }
    // Index-tracking swaps leave `uly` empty and carry the pair only in
    // `instFamily`; one not yet launched — `JP225-USDT-SWAP` in `preopen` —
    // leaves every field empty and names its legs only in the id. Each is
    // tried in turn before giving up.
    raw.uly
        .split_once('-')
        .or_else(|| raw.inst_family.split_once('-'))
        .or_else(|| {
            raw.inst_id
                .trim_end_matches("-SWAP")
                .split_once('-')
                .filter(|(base, quote)| !base.is_empty() && !quote.is_empty())
        })
}

fn option_right(opt_type: &str) -> Option<OptionRight> {
    match opt_type {
        "C" => Some(OptionRight::Call),
        "P" => Some(OptionRight::Put),
        _ => None,
    }
}

fn name_of(base: &str, quote: &str, kind: InstrumentKind) -> String {
    match kind {
        InstrumentKind::Perpetual => format!("{base} / {quote} perpetual"),
        InstrumentKind::Option => format!("{base} / {quote} option"),
        _ => format!("{base} / {quote} future"),
    }
}

fn map_status(raw: &str, inst_id: &str) -> InstrumentStatus {
    match raw {
        "live" | "post_only" => InstrumentStatus::Trading,
        "suspend" | "rebase" | "settling" => InstrumentStatus::Halted,
        "preopen" => InstrumentStatus::PreOpen,
        "expired" => InstrumentStatus::Closed,
        "test" => InstrumentStatus::Test,
        other => {
            tracing::warn!(inst_id, state = other, "unknown okx instrument state");
            InstrumentStatus::Unknown
        }
    }
}

/// Registers every OKX market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct OkxPlugin;

impl Plugin for OkxPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "okx".to_owned(),
            name: "OKX".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "OKX spot, swap, futures and options market data".to_owned(),
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
        let group = context.limit_group("okx");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(swap_source(client.clone())));
        context.register_marketdata_source(Arc::new(futures_source(client.clone())));
        for family in DEFAULT_OPTION_FAMILIES {
            context.register_marketdata_source(Arc::new(option_source(client.clone(), family)));
        }
        // Bar traffic shares the same group as every market data source
        // above: one Binance-scale ban has already
        // happened this project because bar fetching is the request-hungry
        // traffic that a doubled budget would exhaust fastest.
        context.register_bar_source(Arc::new(bar_source_spot(client)));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{Market, map_status, option_source, parse};
    use senken_marketdata::MarketDataSource;
    use senken_marketdata::instrument::{
        InstrumentKind, InstrumentStatus, OptionRight, Settlement,
    };
    use senken_marketdata::source::SourceError;
    use senken_venue::{LimitGroup, VenueClient};

    fn test_client() -> VenueClient {
        VenueClient::new(reqwest::Client::new(), LimitGroup::new("test"))
    }

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/instruments.json");
    const SWAP: &[u8] = include_bytes!("../tests/fixtures/swap.json");
    const FUTURES: &[u8] = include_bytes!("../tests/fixtures/futures.json");
    const OPTION: &[u8] = include_bytes!("../tests/fixtures/option.json");

    #[test]
    fn spot_normalises_to_the_fixed_point_contract() {
        let instruments = parse(SPOT, Market::Spot).unwrap();
        let btc = instruments.iter().find(|i| i.symbol == "BTCUSDT").unwrap();

        assert_eq!(btc.source_symbol, "BTC-USDT");
        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USDT"));
        assert_eq!(btc.status, InstrumentStatus::Trading);
        assert!(btc.contract.is_none());
        assert_eq!((btc.price_scale, btc.tick_size), (1, 1));
        assert_eq!((btc.qty_scale, btc.step_size), (8, 1));
    }

    #[test]
    fn entries_with_unusable_sizes_are_skipped_not_fatal() {
        let instruments = parse(SPOT, Market::Spot).unwrap();
        assert_eq!(instruments.len(), 4);
        assert!(
            instruments.iter().all(|i| i.symbol != "BADUSDT"),
            "a zero lot size is meaningless and must be dropped"
        );
    }

    #[test]
    fn an_increment_in_scientific_notation_is_accepted() {
        // Venues do send `1e-8`; it is a perfectly good step of 0.00000001.
        let instruments = parse(SPOT, Market::Spot).unwrap();
        let sci = instruments.iter().find(|i| i.symbol == "SCIUSDT").unwrap();
        assert_eq!((sci.qty_scale, sci.step_size), (8, 1));
    }

    #[test]
    fn swaps_take_their_pair_from_uly_since_base_ccy_is_empty() {
        // The trap: OKX leaves baseCcy/quoteCcy empty on every derivative.
        let instruments = parse(SWAP, Market::Swap).unwrap();
        let inverse = instruments.iter().find(|i| i.symbol == "BTCUSD").unwrap();

        assert_eq!(inverse.base, "BTC");
        assert_eq!(inverse.quote, "USD");
        assert_eq!(inverse.kind, InstrumentKind::Perpetual);
        assert_eq!(inverse.source_symbol, "BTC-USD-SWAP");

        let contract = inverse.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Inverse);
        assert_eq!(contract.settle, "BTC");
        assert_eq!(contract.expiry, None, "a swap never expires");
        assert_eq!((contract.size_scale, contract.contract_size), (0, 100));
    }

    #[test]
    fn dated_futures_carry_their_expiry() {
        let instruments = parse(FUTURES, Market::Futures).unwrap();
        let dated = instruments.first().unwrap();

        assert_eq!(dated.kind, InstrumentKind::Future);
        assert_eq!(dated.base, "BTC");
        assert!(dated.contract.as_ref().unwrap().expiry.is_some());
        assert!(
            dated.symbol.len() > "BTCUSD".len(),
            "the date must stay in the symbol: {}",
            dated.symbol
        );
    }

    #[test]
    fn options_carry_a_strike_and_a_right() {
        let instruments = parse(OPTION, Market::Option).unwrap();
        let call = instruments
            .iter()
            .find(|i| i.source_symbol.ends_with("-C"))
            .unwrap();

        assert_eq!(call.kind, InstrumentKind::Option);
        let terms = call.contract.as_ref().unwrap().option.as_ref().unwrap();
        assert_eq!(terms.right, OptionRight::Call);
        assert!(terms.strike > 0);
    }

    #[test]
    fn an_application_error_code_is_a_rejection() {
        let body = br#"{"code":"50011","msg":"Rate limit reached","data":[]}"#;
        assert!(matches!(
            parse(body, Market::Spot),
            Err(SourceError::Rejected { reason }) if reason.contains("50011")
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse(b"<html>rate limited</html>", Market::Spot).is_err());
    }

    #[test]
    fn each_option_family_is_its_own_source() {
        let btc = option_source(test_client(), "BTC-USD");
        assert_eq!(btc.id(), "okx-option-btc-usd");
        assert!(btc.url().contains("instFamily=BTC-USD"));
    }

    #[test]
    fn maps_every_documented_state() {
        assert_eq!(map_status("live", "X"), InstrumentStatus::Trading);
        assert_eq!(map_status("post_only", "X"), InstrumentStatus::Trading);
        assert_eq!(map_status("suspend", "X"), InstrumentStatus::Halted);
        assert_eq!(map_status("preopen", "X"), InstrumentStatus::PreOpen);
        assert_eq!(map_status("test", "X"), InstrumentStatus::Test);
        assert_eq!(map_status("something_new", "X"), InstrumentStatus::Unknown);
    }
}
