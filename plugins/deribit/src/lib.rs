//! Deribit market data for Senken: spot, perpetuals, dated futures and
//! options.
//!
//! Deribit lists everything in one document, so this is a single source
//! covering every kind. Two traps it handles: a perpetual carries a
//! year-3000 expiry sentinel rather than none, and combo (multi-leg)
//! instruments are not single tradable contracts, so they are left out.

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
mod book;
mod feed;

pub use bars::{DeribitBarSource, bar_source};

/// Source id of the Deribit market.
pub const SOURCE_ID: &str = "deribit";

const URL: &str = "https://www.deribit.com/api/v2/public/get_instruments?currency=any";

/// Every Deribit instrument: spot, perpetuals, dated futures and options.
#[must_use]
pub fn source(client: VenueClient) -> HttpSource {
    HttpSource::new(SOURCE_ID, "Deribit", URL, client, parse)
}

fn parse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    let response: InstrumentsResponse =
        serde_json::from_slice(body).map_err(SourceError::decode)?;
    if let Some(error) = response.error {
        return Err(SourceError::rejected(format!(
            "code {}: {}",
            error.code, error.message
        )));
    }
    Ok(response
        .result
        .into_iter()
        .filter_map(to_instrument)
        .collect())
}

fn to_instrument(raw: RawInstrument) -> Option<Instrument> {
    let name = &raw.instrument_name;
    // Combos are multi-leg strategies, not instruments with one price.
    let kind = match raw.kind.as_str() {
        "spot" => InstrumentKind::Spot,
        "option" => InstrumentKind::Option,
        "future" if raw.settlement_period == "perpetual" => InstrumentKind::Perpetual,
        "future" => InstrumentKind::Future,
        _ => return None,
    };

    // On an option, `quote_currency` is the premium currency — `BTC` on an
    // inverse BTC option — which would render the pair as `BTC/BTC`.
    // `counter_currency` names the real other leg on every kind.
    let quote = if raw.counter_currency.is_empty() {
        raw.quote_currency.as_str()
    } else {
        raw.counter_currency.as_str()
    };
    if raw.base_currency.is_empty() || quote.is_empty() {
        return skip(SOURCE_ID, name, "missing base or counter currency");
    }
    let Some(price) = raw.tick_size.increment() else {
        return skip(SOURCE_ID, name, "unusable tick_size");
    };
    let Some(qty) = raw.min_trade_amount.increment() else {
        return skip(SOURCE_ID, name, "unusable min_trade_amount");
    };

    let status = if raw.is_active {
        InstrumentStatus::Trading
    } else {
        InstrumentStatus::Closed
    };
    let symbol = normalise_symbol(name, &['-', '_']);

    let instrument = if kind == InstrumentKind::Spot {
        Instrument::spot(symbol, name, raw.base_currency, quote)
    } else {
        let settlement = if raw.instrument_type == "reversed" {
            Settlement::Inverse
        } else {
            Settlement::Linear
        };
        let settle = if raw.settlement_currency.is_empty() {
            quote
        } else {
            raw.settlement_currency.as_str()
        };

        let mut contract = Contract::new(settle, settlement);
        // A perpetual's `expiration_timestamp` is a year-3000 sentinel, so
        // the settlement period is what decides whether an expiry is real.
        if kind != InstrumentKind::Perpetual
            && let Some(expiry) = raw.expiration_timestamp.as_i64().filter(|ms| *ms > 0)
        {
            let Some(expiry) = UnixNanos::from_millis(expiry) else {
                return skip(SOURCE_ID, name, "expiration_timestamp overflowed UnixNanos");
            };
            contract = contract.with_expiry(expiry);
        }
        if let Some((scale, size)) = raw.contract_size.increment() {
            contract = contract.with_contract_size(scale, size);
        }
        if let Some(right) = option_right(&raw.option_type) {
            let (strike_scale, strike) = raw.strike.increment()?;
            contract = contract.with_option(right, strike_scale, strike);
        }

        let display = display_name(&raw, quote, kind);
        Instrument::derivative(symbol, name, raw.base_currency, quote, kind, contract)
            .with_name(display)
    };

    Some(
        instrument
            .with_status(status)
            .with_price_increment(price)
            .with_qty_increment(qty),
    )
}

fn option_right(option_type: &str) -> Option<OptionRight> {
    match option_type {
        "call" => Some(OptionRight::Call),
        "put" => Some(OptionRight::Put),
        _ => None,
    }
}

fn display_name(raw: &RawInstrument, quote: &str, kind: InstrumentKind) -> String {
    let base = &raw.base_currency;
    match kind {
        InstrumentKind::Perpetual => format!("{base} / {quote} perpetual"),
        InstrumentKind::Option => format!("{base} / {quote} option"),
        _ => format!("{base} / {quote} future"),
    }
}

/// Registers the Deribit market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct DeribitPlugin;

impl Plugin for DeribitPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "deribit".to_owned(),
            name: "Deribit".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "Deribit spot, perpetual, futures and options market data".to_owned(),
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
        let group = context.limit_group("deribit");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(source(client.clone())));
        context.register_bar_source(Arc::new(bar_source(
            client.clone(),
            Arc::new(senken_plugin::SystemClock),
        )));
        context.register_book_source(Arc::new(crate::book::book_source(client)));
        context.register_feed_source(Arc::new(crate::feed::DeribitFeedSource::new()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::parse;
    use senken_marketdata::instrument::{InstrumentKind, OptionRight, Settlement};
    use senken_marketdata::source::SourceError;

    const FIXTURE: &[u8] = include_bytes!("../tests/fixtures/instruments.json");

    #[test]
    fn a_perpetual_ignores_its_sentinel_expiry() {
        let instruments = parse(FIXTURE).unwrap();
        let perp = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Perpetual)
            .expect("the fixture carries a perpetual");

        assert_eq!(
            perp.contract.as_ref().unwrap().expiry,
            None,
            "the year-3000 sentinel is not a real expiry"
        );
    }

    #[test]
    fn a_reversed_contract_is_inverse() {
        let instruments = parse(FIXTURE).unwrap();
        let inverse = instruments
            .iter()
            .find(|i| {
                i.contract
                    .as_ref()
                    .is_some_and(|c| c.settlement == Settlement::Inverse)
            })
            .expect("the fixture carries a reversed contract");
        assert!(inverse.kind.is_derivative());
    }

    #[test]
    fn an_option_is_paired_against_its_counter_currency() {
        // `quote_currency` on a BTC option is the premium currency, BTC.
        // Reading it as the pair's other leg renders the instrument as
        // `BTC/BTC`, which is no pair at all.
        let instruments = parse(FIXTURE).unwrap();
        let option = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Option)
            .expect("the fixture carries an option");

        assert_eq!(option.base, "BTC");
        assert_eq!(option.quote, "USD", "the premium currency is not the quote");
        assert_eq!(
            option.contract.as_ref().unwrap().settle,
            "BTC",
            "an inverse option still settles in the premium currency"
        );
    }

    #[test]
    fn options_carry_a_strike_even_in_scientific_notation() {
        // Deribit sends about half its strikes as `6.9e4`.
        let instruments = parse(FIXTURE).unwrap();
        let option = instruments
            .iter()
            .find(|i| i.kind == InstrumentKind::Option)
            .expect("the fixture carries an option");

        let terms = option.contract.as_ref().unwrap().option.as_ref().unwrap();
        assert!(terms.strike > 0, "a strike of zero means the parse failed");
        assert!(matches!(terms.right, OptionRight::Call | OptionRight::Put));
        assert!(option.contract.as_ref().unwrap().expiry.is_some());
    }

    #[test]
    fn combos_are_not_instruments() {
        let instruments = parse(FIXTURE).unwrap();
        assert!(
            instruments
                .iter()
                .all(|i| !i.source_symbol.contains("_combo")),
            "multi-leg combos have no single price and must be left out"
        );
    }

    #[test]
    fn a_json_rpc_error_is_a_rejection() {
        let body = br#"{"jsonrpc":"2.0","error":{"code":10009,"message":"not_enough_funds"}}"#;
        assert!(matches!(
            parse(body),
            Err(SourceError::Rejected { reason }) if reason.contains("10009")
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse(b"<html>nope</html>").is_err());
    }
}
