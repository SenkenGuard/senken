//! Upbit market data for Senken: spot pairs.
//!
//! Upbit lists no derivatives, and its market document is unusually thin —
//! four keys, none of them numeric.
//!
//! # Two things to know before trusting this data
//!
//! **The pair is written backwards.** Upbit's `market` field is
//! `QUOTE-BASE`: `KRW-BTC` is Bitcoin priced in won, not the reverse. Read
//! left to right like every other venue and every instrument comes out
//! inverted.
//!
//! **Upbit publishes no price tick and no quantity step**, on any public
//! endpoint. Its won-quoted prices move on a banded table — finer ticks at
//! low prices, coarser at high ones — which a single per-instrument
//! increment cannot express in the first place. Every instrument here
//! therefore carries [`UNPUBLISHED_INCREMENT`]; the symbols and their legs
//! are real, **the increments are placeholders and must not be used to
//! round an order.**

use std::sync::Arc;

use senken_marketdata::instrument::{Instrument, InstrumentStatus};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};
use serde::Deserialize;

mod bars;
mod book;
mod feed;

pub use bars::{UpbitBarSource, bar_source};

/// Source id of the Upbit market.
pub const SOURCE_ID: &str = "upbit";

const URL: &str = "https://api.upbit.com/v1/market/all?isDetails=true";

/// The increment stored because Upbit publishes none. A scale of zero and a
/// size of one is an obvious stand-in rather than a plausible-looking tick.
/// See the module docs.
pub const UNPUBLISHED_INCREMENT: (u8, i64) = (0, 1);

/// One entry of `GET /v1/market/all`.
#[derive(Debug, Deserialize)]
struct RawMarket {
    /// `QUOTE-BASE`, e.g. `KRW-BTC`.
    market: String,
    #[serde(default)]
    english_name: String,
    #[serde(default)]
    market_event: Option<MarketEvent>,
}

#[derive(Debug, Deserialize)]
struct MarketEvent {
    /// Upbit's investment-warning flag, the only status signal it offers.
    #[serde(default)]
    warning: bool,
}

/// The Upbit spot market.
#[must_use]
pub fn source(client: VenueClient) -> HttpSource {
    HttpSource::new(SOURCE_ID, "Upbit", URL, client, parse)
}

fn parse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let markets: Vec<RawMarket> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    Ok(markets.into_iter().filter_map(to_instrument).collect())
}

fn to_instrument(raw: RawMarket) -> Option<Instrument> {
    // `KRW-BTC` is quote first, base second — the reverse of every other
    // venue in this workspace.
    let Some((quote, base)) = raw.market.split_once('-') else {
        return skip(SOURCE_ID, &raw.market, "market has no separator");
    };
    let (base, quote) = (base.to_owned(), quote.to_owned());
    if base.is_empty() || quote.is_empty() {
        return skip(SOURCE_ID, &raw.market, "empty base or quote");
    }

    // A warning flag is the only state Upbit exposes; everything listed is
    // otherwise trading.
    let status = if raw.market_event.is_some_and(|event| event.warning) {
        InstrumentStatus::Halted
    } else {
        InstrumentStatus::Trading
    };

    let symbol = normalise_symbol(&raw.market, &['-']);
    // The display name reads `quote`, so it is built before the legs move
    // into the instrument.
    let name = (!raw.english_name.is_empty()).then(|| format!("{} / {quote}", raw.english_name));
    let instrument = Instrument::spot(symbol, raw.market, base, quote)
        .with_status(status)
        .with_price_increment(UNPUBLISHED_INCREMENT)
        .with_qty_increment(UNPUBLISHED_INCREMENT);

    Some(match name {
        Some(name) => instrument.with_name(name),
        None => instrument,
    })
}

/// Registers the Upbit market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct UpbitPlugin;

impl Plugin for UpbitPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "upbit".to_owned(),
            name: "Upbit".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Upbit spot market data (no published increments)".to_owned(),
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
        let group = context.limit_group("upbit");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(source(client.clone())));
        context.register_bar_source(Arc::new(bar_source(
            client.clone(),
            Arc::new(senken_plugin::SystemClock),
        )));
        context.register_book_source(Arc::new(crate::book::book_source(client)));
        context.register_feed_source(Arc::new(crate::feed::UpbitFeedSource::new()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{UNPUBLISHED_INCREMENT, parse};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus};

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/markets.json");

    #[test]
    fn the_pair_is_read_quote_first() {
        // `KRW-BTC` is Bitcoin priced in won. Reading it left to right
        // would make every Upbit instrument inside out.
        let instruments = parse(FIXTURE).unwrap();
        let btc = instruments
            .iter()
            .find(|i| i.source_symbol == "KRW-BTC")
            .expect("the fixture carries KRW-BTC");

        assert_eq!(btc.base, "BTC", "the base comes second");
        assert_eq!(btc.quote, "KRW", "the quote comes first");
        assert_eq!(btc.kind, InstrumentKind::Spot);
    }

    #[test]
    fn a_warning_flag_is_the_only_status_upbit_offers() {
        let instruments = parse(FIXTURE).unwrap();
        assert!(
            instruments
                .iter()
                .any(|i| i.status == InstrumentStatus::Trading)
        );
    }

    #[test]
    fn every_increment_is_the_documented_placeholder() {
        for instrument in parse(FIXTURE).unwrap() {
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
        assert!(parse(b"<html>nope</html>").is_err());
    }
}
