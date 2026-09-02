//! Bitfinex market data for Senken: spot pairs and perpetual futures.
//!
//! # Bitfinex publishes no price increment
//!
//! This venue is the exception to the fixed-point contract every other
//! adapter satisfies from real venue data. Bitfinex's v2 configuration
//! endpoints carry no tick size at all, and the legacy v1 endpoint reports
//! `price_precision: 5` for *every* symbol — five **significant figures**,
//! not five decimal places, so the real increment moves with the price:
//! five significant figures is a tick of `1` on a $100,000 asset and
//! `0.0000001` on a $0.001 one. A single per-instrument increment cannot
//! express that.
//!
//! Rather than invent a number that looks authoritative, this adapter
//! stores [`UNPUBLISHED_INCREMENT`] on every Bitfinex instrument and says
//! so here. The symbols, their base and quote, and their market type are
//! all real; **the increments are placeholders and must not be used to
//! round an order.**

use std::sync::Arc;

use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

mod bars;
mod book;
mod feed;

pub use bars::{BitfinexBarSource, bar_source, bar_source_spot};

/// Source id of the spot market.
pub const SPOT_ID: &str = "bitfinex-spot";
/// Source id of the perpetual market.
pub const PERP_ID: &str = "bitfinex-perp";

const SPOT_URL: &str = "https://api-pub.bitfinex.com/v2/conf/pub:list:pair:exchange";
const PERP_URL: &str = "https://api-pub.bitfinex.com/v2/conf/pub:list:pair:futures";

/// The increment stored when a venue publishes none.
///
/// A scale of zero and a size of one — "one whole unit" — is the least
/// misleading placeholder available: it is obviously a stand-in rather than
/// a plausible-looking tick such as `0.00000001`. See the module docs.
pub const UNPUBLISHED_INCREMENT: (u8, i64) = (0, 1);

/// Bitfinex marks both legs of a derivative with an `F0` suffix:
/// `BTCF0:USTF0` is the BTC perpetual quoted in Tether.
const DERIVATIVE_SUFFIX: &str = "F0";

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "Bitfinex Spot", SPOT_URL, client, parse_spot)
}

/// The perpetual market. Bitfinex lists no dated futures.
#[must_use]
pub fn perp_source(client: VenueClient) -> HttpSource {
    HttpSource::new(PERP_ID, "Bitfinex Perpetuals", PERP_URL, client, parse_perp)
}

/// Every configuration endpoint answers with a one-element array wrapping
/// the real payload.
fn symbols(body: &[u8]) -> Result<Vec<String>, SourceError> {
    let wrapper: Vec<Vec<String>> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(wrapper.into_iter().next().unwrap_or_default())
}

fn parse_spot(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    Ok(symbols(body)?
        .into_iter()
        .filter_map(|symbol| to_instrument(&symbol, false))
        .collect())
}

fn parse_perp(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    Ok(symbols(body)?
        .into_iter()
        .filter_map(|symbol| to_instrument(&symbol, true))
        .collect())
}

fn to_instrument(symbol: &str, derivative: bool) -> Option<Instrument> {
    let source = if derivative { PERP_ID } else { SPOT_ID };
    let Some((base, quote)) = split_pair(symbol) else {
        return skip(source, symbol, "cannot split the pair");
    };

    let instrument = if derivative {
        // Every Bitfinex perpetual is quoted and settled in USDt.
        Instrument::derivative(
            normalise_symbol(symbol, &[':']),
            symbol,
            &base,
            &quote,
            InstrumentKind::Perpetual,
            Contract::new(&quote, Settlement::Linear),
        )
        .with_name(format!("{base} / {quote} perpetual"))
    } else {
        Instrument::spot(normalise_symbol(symbol, &[':']), symbol, &base, &quote)
    };

    Some(
        instrument
            // The configuration list holds only live pairs.
            .with_status(InstrumentStatus::Trading)
            .with_price_increment(UNPUBLISHED_INCREMENT)
            .with_qty_increment(UNPUBLISHED_INCREMENT),
    )
}

/// Splits a Bitfinex pair into base and quote.
///
/// Longer codes are separated by a colon (`AAVE:USD`); three-letter ones
/// are simply concatenated (`ADABTC`). A derivative carries `F0` on both
/// legs, which names no currency and is stripped.
fn split_pair(symbol: &str) -> Option<(String, String)> {
    let (base, quote) = match symbol.split_once(':') {
        Some(pair) => pair,
        None if symbol.len() == 6 => symbol.split_at(3),
        None => return None,
    };
    let base = base.strip_suffix(DERIVATIVE_SUFFIX).unwrap_or(base);
    let quote = quote.strip_suffix(DERIVATIVE_SUFFIX).unwrap_or(quote);
    if base.is_empty() || quote.is_empty() {
        return None;
    }
    Some((base.to_uppercase(), quote.to_uppercase()))
}

/// Registers both Bitfinex markets with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct BitfinexPlugin;

impl Plugin for BitfinexPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "bitfinex".to_owned(),
            name: "Bitfinex".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Bitfinex spot and perpetual market data (no published increments)"
                .to_owned(),
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
        let group = context.limit_group("bitfinex");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(perp_source(client.clone())));
        // One candles endpoint and one depth endpoint serve both markets
        // — each takes the symbol in its path and nothing else, and the
        // perpetual answers the identical row and level shapes.
        // Confirmed live 2026-09-02 against `tBTCF0:USTF0`.
        //
        // Neither endpoint carries a timestamp of its own, so both need
        // the same real-time clock (see each module's own docs).
        for market in [SPOT_ID, PERP_ID] {
            context.register_bar_source(Arc::new(bars::bar_source(
                market,
                client.clone(),
                Arc::new(senken_plugin::SystemClock),
            )));
            context.register_book_source(Arc::new(crate::book::book_source(
                market,
                client.clone(),
                Arc::new(senken_plugin::SystemClock),
            )));
        }
        let _ = &client;
        context.register_feed_source(Arc::new(crate::feed::BitfinexFeedSource::new()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{UNPUBLISHED_INCREMENT, parse_perp, parse_spot, split_pair};
    use senken_marketdata::instrument::{InstrumentKind, Settlement};

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const PERP: &[u8] = include_bytes!("../tests/fixtures/perp.json");

    #[test]
    fn colon_and_concatenated_pairs_both_split() {
        assert_eq!(
            split_pair("AAVE:USD"),
            Some(("AAVE".into(), "USD".into())),
            "longer codes are colon separated"
        );
        assert_eq!(
            split_pair("ADABTC"),
            Some(("ADA".into(), "BTC".into())),
            "three-letter codes are simply concatenated"
        );
        assert_eq!(split_pair("TOOLONGNOSEP"), None);
    }

    #[test]
    fn a_derivative_loses_the_f0_marker_on_both_legs() {
        assert_eq!(
            split_pair("BTCF0:USTF0"),
            Some(("BTC".into(), "UST".into())),
            "F0 marks a derivative leg and names no currency"
        );
    }

    #[test]
    fn spot_pairs_are_listed() {
        let instruments = parse_spot(SPOT).unwrap();
        assert!(!instruments.is_empty());
        assert!(instruments.iter().all(|i| i.kind == InstrumentKind::Spot));
        assert!(instruments.iter().all(|i| i.contract.is_none()));
    }

    #[test]
    fn perpetuals_are_linear_and_settle_in_their_quote() {
        let instruments = parse_perp(PERP).unwrap();
        let perp = instruments.first().expect("the fixture carries a contract");

        assert_eq!(perp.kind, InstrumentKind::Perpetual);
        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.settle, perp.quote);
    }

    #[test]
    fn every_increment_is_the_documented_placeholder() {
        // Bitfinex publishes none; this asserts the adapter never invents
        // one that could be mistaken for venue data.
        for instrument in parse_spot(SPOT)
            .unwrap()
            .iter()
            .chain(&parse_perp(PERP).unwrap())
        {
            assert_eq!(
                (instrument.price_scale, instrument.tick_size),
                UNPUBLISHED_INCREMENT
            );
            assert_eq!(
                (instrument.qty_scale, instrument.step_size),
                UNPUBLISHED_INCREMENT
            );
        }
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
    }
}
