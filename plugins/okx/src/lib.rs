//! OKX market data for Senken: spot, perpetual swaps, dated futures and
//! options.
//!
//! Every market's catalog (and, for spot/swap/futures, its bars) is served
//! by a `wasm32-wasip2` component — see [`Plugin::venue_components`] — not
//! by the [`MarketDataSource`]/[`BarSource`] functions this crate still
//! defines ([`spot_source`], [`swap_source`], [`futures_source`],
//! [`option_source`], [`bar_source`]): those are kept as plain library
//! functions purely so `tests/wasm_parity.rs` has a native reference to
//! compare each component's output against. Options are listed per
//! underlying family — OKX refuses to enumerate them all at once — so
//! [`option_source`] takes the family; `okx-option-btc-usd` and
//! `okx-option-eth-usd` are the two components this plugin actually
//! embeds, one per liquid family.
//!
//! [`MarketDataSource`]: senken_marketdata::MarketDataSource
//! [`BarSource`]: senken_plugin::BarSource
//! [`Plugin::venue_components`]: senken_plugin::Plugin::venue_components

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, OptionRight, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient};

mod bars;
mod book;
mod feed;

pub use crate::bars::{OkxBarSource, bar_source};

/// Source id of the spot market.
pub const SPOT_ID: &str = "okx-spot";
/// Source id of the perpetual swap market.
pub const SWAP_ID: &str = "okx-swap";
/// Source id of the dated futures market.
pub const FUTURES_ID: &str = "okx-futures";

const BASE_URL: &str = "https://www.okx.com/api/v5/public/instruments";

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
    parse(body, okx_core::Market::Spot)
}

fn parse_swap(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, okx_core::Market::Swap)
}

fn parse_futures(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, okx_core::Market::Futures)
}

fn parse_option(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse(body, okx_core::Market::Option)
}

/// Decodes an `instruments` document through `okx-core`'s shared parser —
/// the same one `plugins/okx/wasm` calls for the markets it serves — and
/// adapts its neutral result onto the domain [`Instrument`] every
/// `senken-marketdata` consumer speaks. This crate no longer parses OKX's
/// wire shape itself: `okx-core` is the one place either side of the
/// native/wasm boundary makes sense of an OKX document.
fn parse(body: &[u8], market: okx_core::Market) -> Result<Vec<Instrument>, SourceError> {
    let instruments = okx_core::parse_instruments(body, market).map_err(to_source_error)?;
    Ok(instruments.into_iter().map(to_domain_instrument).collect())
}

fn to_source_error(error: okx_core::CoreError) -> SourceError {
    match error {
        okx_core::CoreError::Decode(message) => SourceError::decode(message),
        okx_core::CoreError::Rejected(message) => SourceError::rejected(message),
    }
}

fn to_domain_instrument(instrument: okx_core::Instrument) -> Instrument {
    let built = match instrument.contract {
        Some(contract) => Instrument::derivative(
            instrument.symbol,
            instrument.source_symbol,
            instrument.base,
            instrument.quote,
            to_domain_kind(instrument.kind),
            to_domain_contract(contract),
        ),
        None => Instrument::spot(
            instrument.symbol,
            instrument.source_symbol,
            instrument.base,
            instrument.quote,
        ),
    };
    built
        .with_name(instrument.name)
        .with_status(to_domain_status(instrument.status))
        .with_price_increment((instrument.price_scale, instrument.tick_size))
        .with_qty_increment((instrument.qty_scale, instrument.step_size))
}

fn to_domain_kind(kind: okx_core::InstrumentKind) -> InstrumentKind {
    match kind {
        okx_core::InstrumentKind::Spot => InstrumentKind::Spot,
        okx_core::InstrumentKind::Future => InstrumentKind::Future,
        okx_core::InstrumentKind::Option => InstrumentKind::Option,
        okx_core::InstrumentKind::Perpetual => InstrumentKind::Perpetual,
    }
}

fn to_domain_status(status: okx_core::InstrumentStatus) -> InstrumentStatus {
    match status {
        okx_core::InstrumentStatus::Trading => InstrumentStatus::Trading,
        okx_core::InstrumentStatus::Halted => InstrumentStatus::Halted,
        okx_core::InstrumentStatus::PreOpen => InstrumentStatus::PreOpen,
        okx_core::InstrumentStatus::Closed => InstrumentStatus::Closed,
        okx_core::InstrumentStatus::Test => InstrumentStatus::Test,
        okx_core::InstrumentStatus::Unknown => InstrumentStatus::Unknown,
    }
}

fn to_domain_settlement(settlement: okx_core::Settlement) -> Settlement {
    match settlement {
        okx_core::Settlement::Linear => Settlement::Linear,
        okx_core::Settlement::Inverse => Settlement::Inverse,
        okx_core::Settlement::Quanto => Settlement::Quanto,
    }
}

fn to_domain_option_right(right: okx_core::OptionRight) -> OptionRight {
    match right {
        okx_core::OptionRight::Call => OptionRight::Call,
        okx_core::OptionRight::Put => OptionRight::Put,
    }
}

fn to_domain_contract(contract: okx_core::Contract) -> Contract {
    let mut built = Contract::new(contract.settle, to_domain_settlement(contract.settlement));
    if let Some(expiry) = contract.expiry {
        built = built.with_expiry(expiry);
    }
    built = built.with_contract_size(contract.size_scale, contract.contract_size);
    if let Some(option) = contract.option {
        built = built.with_option(
            to_domain_option_right(option.right),
            option.strike_scale,
            option.strike,
        );
    }
    built
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
            contributes: senken_plugin::parse_static_contributions(include_str!(
                "../senken-plugin.json"
            ))
            .expect("senken-plugin.json is well-formed"),
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
        // No market's `MarketDataSource`/`BarSource` registers here
        // anymore: `Self::venue_components` hands the runtime all five of
        // this venue's `wasm32-wasip2` components instead (spot, swap,
        // futures, and the two liquid option families), and the runtime
        // registers each market's catalog — and, for spot/swap/futures,
        // its bars — from that dynamic source. See this method's own
        // module docs and `crate::venue_components`. `swap_source`/
        // `futures_source`/`option_source`/`bar_source` below are kept as
        // plain library functions (not registered as this plugin's live
        // sources) purely so `tests/wasm_parity.rs` still has a native
        // reference to compare each component's output against — the same
        // reason `spot_source` was kept when spot made this same move.
        //
        // Every order-book and the live feed stay native below: neither
        // is exported by `wit/senken.wit`'s `venue-plugin` world yet.
        for market in [SPOT_ID, SWAP_ID, FUTURES_ID] {
            context
                .register_book_source(Arc::new(crate::book::book_source(market, client.clone())));
        }
        // Depth and the live stream, declared the same way as everything
        // above rather than wired into the HTTP layer by hand — a venue
        // that serves neither simply registers neither.
        let _ = &client;
        context.register_feed_source(Arc::new(crate::feed::OkxFeedSource::new()));
        Ok(())
    }

    fn venue_components(&self) -> Vec<&'static [u8]> {
        // Built by `plugins/build-venue.sh okx` (spot) and
        // `plugins/build-venue.sh okx <wasm-dir> <crate-name>` (the other
        // four — see that script's own usage docs) into `dist/`,
        // gitignored, and embedded here rather than read from disk at
        // startup — see this crate's `wasm*/` directories for the
        // components themselves and this method's caller
        // (`senken_runtime::RuntimeBuilder::build`) for what registering
        // them replaces. Each `include_bytes!` fails to compile this crate
        // if its artifact is missing, which is why CI (and any local build
        // of this crate) runs the build script five times first.
        vec![
            include_bytes!("../dist/okx-venue.wasm"),
            include_bytes!("../dist/okx-venue-swap.wasm"),
            include_bytes!("../dist/okx-venue-futures.wasm"),
            include_bytes!("../dist/okx-venue-option-btc-usd.wasm"),
            include_bytes!("../dist/okx-venue-option-eth-usd.wasm"),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::{option_source, parse};
    use okx_core::Market;
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

        // The same function `feed.rs`'s live decoder calls on this exact
        // `instId` — see that module's own
        // `the_live_decoder_normalises_a_perpetual_swaps_instid_the_same_way_the_catalog_does`.
        assert_eq!(
            inverse.symbol,
            okx_core::normalise_okx_symbol(&inverse.source_symbol)
        );
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
        // The fixture's own recorded `stk` (`"70000"`, a bare integer) and
        // its scale (`0`, since it carries no fractional digits) —
        // asserted exactly rather than `> 0`, which a wrong scale would
        // still satisfy.
        assert_eq!((terms.strike_scale, terms.strike), (0, 70_000));
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

    // `map_status`'s own case-for-case coverage now lives in `okx-core`'s
    // test module (`maps_every_documented_state`), alongside the function
    // itself — this crate's tests above still prove the *adapter* from
    // `okx-core`'s neutral result onto the domain `Instrument` (`status`,
    // among the rest, crossing that adapter correctly), just not the state
    // string mapping a second time.
}
