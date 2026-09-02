//! HTX (formerly Huobi) market data for Senken: spot, linear derivatives
//! and inverse derivatives.
//!
//! Four endpoints across two hosts. Whether a contract is linear or inverse
//! is decided by *which endpoint answered*, never by a field, so each is
//! its own source with its own settlement fixed at construction.

use std::sync::Arc;

use senken_core::UnixNanos;
use senken_marketdata::instrument::{
    Contract, Instrument, InstrumentKind, InstrumentStatus, Settlement,
};
use senken_marketdata::source::SourceError;
use senken_plugin::{HttpActivationContext, Plugin, PluginError, PluginManifest};
use senken_venue::{HttpSource, VenueClient, normalise_symbol, skip};

use crate::api::{Envelope, RawContract, RawSymbol};

mod api;
mod bars;
mod book;
mod feed;

pub use bars::{HtxBarSource, bar_source_derivative, bar_source_spot};

/// The host every HTX derivative market is served from — spot alone
/// lives on `api.huobi.pro`.
const DERIVATIVE_BASE: &str = "https://api.hbdm.com";

/// Source id of the spot market.
pub const SPOT_ID: &str = "htx-spot";
/// Source id of the linear (USDT-margined) derivatives market.
pub const LINEAR_ID: &str = "htx-linear";
/// Source id of the inverse perpetual market.
pub const INVERSE_SWAP_ID: &str = "htx-inverse-swap";
/// Source id of the inverse dated futures market.
pub const INVERSE_FUTURES_ID: &str = "htx-inverse-futures";

const SPOT_URL: &str = "https://api.huobi.pro/v1/settings/common/market-symbols";
const LINEAR_URL: &str =
    "https://api.hbdm.com/linear-swap-api/v1/swap_contract_info?business_type=all";
const INVERSE_SWAP_URL: &str = "https://api.hbdm.com/swap-api/v1/swap_contract_info";
const INVERSE_FUTURES_URL: &str = "https://api.hbdm.com/api/v1/contract_contract_info";

/// The spot market.
#[must_use]
pub fn spot_source(client: VenueClient) -> HttpSource {
    HttpSource::new(SPOT_ID, "HTX Spot", SPOT_URL, client, parse_spot)
}

/// Linear perpetuals and linear dated futures.
#[must_use]
pub fn linear_source(client: VenueClient) -> HttpSource {
    HttpSource::new(LINEAR_ID, "HTX Linear", LINEAR_URL, client, parse_linear)
}

/// Inverse perpetuals.
#[must_use]
pub fn inverse_swap_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        INVERSE_SWAP_ID,
        "HTX Inverse Swap",
        INVERSE_SWAP_URL,
        client,
        parse_inverse,
    )
}

/// Inverse dated futures.
#[must_use]
pub fn inverse_futures_source(client: VenueClient) -> HttpSource {
    HttpSource::new(
        INVERSE_FUTURES_ID,
        "HTX Inverse Futures",
        INVERSE_FUTURES_URL,
        client,
        parse_inverse,
    )
}

fn decode<T: serde::de::DeserializeOwned>(body: &[u8]) -> Result<Vec<T>, SourceError> {
    let envelope: Envelope<T> = serde_json::from_slice(body).map_err(SourceError::decode)?;
    if !envelope.status.is_empty() && envelope.status != "ok" {
        return Err(SourceError::rejected(format!(
            "{}: {}",
            envelope.status, envelope.err_msg
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
    if raw.bc.is_empty() || raw.qc.is_empty() {
        return skip(SPOT_ID, &raw.symbol, "missing bc or qc");
    }
    // HTX spot publishes only decimal-place counts.
    let Some(price) = raw.pp.precision() else {
        return skip(SPOT_ID, &raw.symbol, "unusable pp");
    };
    let Some(qty) = raw.ap.precision() else {
        return skip(SPOT_ID, &raw.symbol, "unusable ap");
    };

    let status = match raw.state.as_str() {
        "online" => InstrumentStatus::Trading,
        "suspend" | "pre-online" => InstrumentStatus::Halted,
        "offline" | "transfer-board" | "delisted" => InstrumentStatus::Closed,
        _ => InstrumentStatus::Unknown,
    };

    // Everything HTX spot sends is lower case; the shared normaliser
    // upper-cases it back into the cross-venue form.
    Some(
        Instrument::spot(
            normalise_symbol(&raw.symbol, &['-']),
            raw.symbol,
            raw.bc.to_uppercase(),
            raw.qc.to_uppercase(),
        )
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

fn parse_linear(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse_contracts(body, Settlement::Linear)
}

fn parse_inverse(body: &[u8]) -> Result<Vec<Instrument>, SourceError> {
    parse_contracts(body, Settlement::Inverse)
}

fn parse_contracts(body: &[u8], settlement: Settlement) -> Result<Vec<Instrument>, SourceError> {
    Ok(decode::<RawContract>(body)?
        .into_iter()
        .filter_map(|raw| contract_instrument(raw, settlement))
        .collect())
}

fn contract_instrument(raw: RawContract, settlement: Settlement) -> Option<Instrument> {
    let source = if settlement == Settlement::Inverse {
        INVERSE_SWAP_ID
    } else {
        LINEAR_ID
    };
    let Some((base, quote)) = contract_pair(&raw, settlement) else {
        return skip(source, &raw.contract_code, "cannot resolve base and quote");
    };
    let Some(price) = raw.price_tick.increment() else {
        return skip(source, &raw.contract_code, "unusable price_tick");
    };

    // HTX quantities are whole contracts on every derivative market; the
    // documents carry no quantity step of their own.
    let qty = (0, 1);

    let expiry = raw.delivery_time.as_i64().filter(|ms| *ms > 0);
    // `contract_type` is `swap` on a perpetual and a tenor name otherwise;
    // the inverse perpetual document omits the field entirely.
    let dated = expiry.is_some()
        || (!raw.contract_type.is_empty() && !raw.contract_type.eq_ignore_ascii_case("swap"));
    let kind = if dated {
        InstrumentKind::Future
    } else {
        InstrumentKind::Perpetual
    };

    let settle = if settlement == Settlement::Inverse {
        base.clone()
    } else if raw.trade_partition.is_empty() {
        quote.clone()
    } else {
        raw.trade_partition.clone()
    };

    let mut contract = Contract::new(settle, settlement);
    if let Some(expiry) = expiry {
        let Some(expiry) = UnixNanos::from_millis(expiry) else {
            return skip(
                source,
                &raw.contract_code,
                "delivery_time overflowed UnixNanos",
            );
        };
        contract = contract.with_expiry(expiry);
    }
    if let Some((scale, size)) = raw.contract_size.increment() {
        contract = contract.with_contract_size(scale, size);
    }

    let status = if raw.contract_status == 1 {
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
            normalise_symbol(&raw.contract_code, &['-']),
            raw.contract_code,
            &base,
            &quote,
            kind,
            contract,
        )
        .with_name(name)
        .with_status(status)
        .with_price_increment(price)
        .with_qty_increment(qty),
    )
}

/// Base and quote for a contract. HTX gives neither directly: `symbol` is
/// the base coin alone, and the pair has to come from `pair`, from
/// `contract_code`, or — on inverse dated futures such as `BTC260904` —
/// from the base coin plus the market's implied quote.
fn contract_pair(raw: &RawContract, settlement: Settlement) -> Option<(String, String)> {
    for candidate in [raw.pair.as_str(), raw.contract_code.as_str()] {
        if let Some((base, quote)) = candidate.split_once('-')
            && !base.is_empty()
            && !quote.is_empty()
        {
            // A dated linear contract is `BTC-USDT-260904`; keep the pair.
            let quote = quote.split('-').next().unwrap_or(quote);
            return Some((base.to_uppercase(), quote.to_uppercase()));
        }
    }
    if raw.symbol.is_empty() {
        return None;
    }
    // Inverse dated futures are coin-margined and quoted in USD.
    let quote = if settlement == Settlement::Inverse {
        "USD"
    } else {
        return None;
    };
    Some((raw.symbol.to_uppercase(), quote.to_owned()))
}

/// Registers every HTX market with the Senken runtime.
#[derive(Debug, Default, Clone, Copy)]
pub struct HtxPlugin;

impl Plugin for HtxPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: "htx".to_owned(),
            name: "HTX".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "HTX spot, linear and inverse derivatives market data".to_owned(),
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
        let group = context.limit_group("htx");
        let client = context.venue_client(&group)?;
        context.register_marketdata_source(Arc::new(spot_source(client.clone())));
        context.register_marketdata_source(Arc::new(linear_source(client.clone())));
        context.register_marketdata_source(Arc::new(inverse_swap_source(client.clone())));
        context.register_marketdata_source(Arc::new(inverse_futures_source(client.clone())));
        // Spot only: the three derivative markets live on a different host
        // with a different path per market — see `bars`' own module docs.
        context.register_bar_source(Arc::new(bar_source_spot(
            client.clone(),
            Arc::new(senken_plugin::SystemClock),
        )));
        // Spot only: the three derivative markets' depth endpoints were
        // not recorded live this session (see `book`'s own module docs).
        context.register_book_source(Arc::new(crate::book::book_source_spot(client.clone())));

        // The three derivative markets each have their own host path and
        // their own name for the symbol parameter, but answer the same
        // row and level shapes as spot — confirmed live by fetching all
        // four.
        for (source_id, path, symbol_param) in [
            (LINEAR_ID, "linear-swap-ex", "contract_code"),
            (INVERSE_SWAP_ID, "swap-ex", "contract_code"),
            (INVERSE_FUTURES_ID, "", "symbol"),
        ] {
            let prefix = if path.is_empty() {
                DERIVATIVE_BASE.to_owned()
            } else {
                format!("{DERIVATIVE_BASE}/{path}")
            };
            context.register_bar_source(Arc::new(crate::bars::bar_source_derivative(
                source_id,
                format!("{prefix}/market/history/kline"),
                symbol_param,
                client.clone(),
                Arc::new(senken_plugin::SystemClock),
            )));
            context.register_book_source(Arc::new(crate::book::book_source_derivative(
                source_id,
                format!("{prefix}/market/depth"),
                symbol_param,
                client.clone(),
            )));
        }

        // Four markets, four sockets: only spot lives on `api.huobi.pro`,
        // and each derivative host carries only its own market.
        for (source_id, ws) in [
            (LINEAR_ID, "wss://api.hbdm.com/linear-swap-ws"),
            (INVERSE_SWAP_ID, "wss://api.hbdm.com/swap-ws"),
            (INVERSE_FUTURES_ID, "wss://api.hbdm.com/ws"),
        ] {
            context.register_feed_source(Arc::new(crate::feed::HtxFeedSource::for_market(
                source_id, ws,
            )));
        }
        let _ = &client;
        context.register_feed_source(Arc::new(crate::feed::HtxFeedSource::new()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_inverse, parse_linear, parse_spot};
    use senken_marketdata::instrument::{InstrumentKind, InstrumentStatus, Settlement};
    use senken_marketdata::source::SourceError;

    const SPOT: &[u8] = include_bytes!("../tests/fixtures/spot.json");
    const LINEAR: &[u8] = include_bytes!("../tests/fixtures/linear.json");
    const INVERSE: &[u8] = include_bytes!("../tests/fixtures/inverse_swap.json");
    const INVERSE_DATED: &[u8] = include_bytes!("../tests/fixtures/inverse_futures.json");

    #[test]
    fn spot_reads_two_letter_keys_and_upper_cases_them() {
        let instruments = parse_spot(SPOT).unwrap();
        let btc = instruments
            .iter()
            .find(|i| i.source_symbol == "btcusdt")
            .expect("the fixture carries btcusdt");

        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USDT"));
        assert_eq!(btc.symbol, "BTCUSDT");
        assert_eq!(btc.status, InstrumentStatus::Trading);
        // pp 2 → a tick of 1 at scale 2.
        assert_eq!((btc.price_scale, btc.tick_size), (2, 1));
    }

    #[test]
    fn a_linear_contract_takes_its_pair_from_contract_code() {
        // `symbol` is only the base coin, so reading it as the pair would
        // leave every contract without a quote.
        let instruments = parse_linear(LINEAR).unwrap();
        let btc = instruments
            .iter()
            .find(|i| i.source_symbol == "BTC-USDT")
            .expect("the fixture carries BTC-USDT");

        assert_eq!((btc.base.as_str(), btc.quote.as_str()), ("BTC", "USDT"));
        assert_eq!(btc.kind, InstrumentKind::Perpetual);
        let contract = btc.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Linear);
        assert_eq!(contract.settle, "USDT");
    }

    #[test]
    fn the_endpoint_decides_inverse_since_no_field_says_so() {
        let instruments = parse_inverse(INVERSE).unwrap();
        let perp = instruments.first().expect("the fixture carries a contract");

        let contract = perp.contract.as_ref().unwrap();
        assert_eq!(contract.settlement, Settlement::Inverse);
        assert_eq!(contract.settle, perp.base);
        assert_eq!(
            perp.kind,
            InstrumentKind::Perpetual,
            "the inverse swap document omits contract_type entirely"
        );
    }

    #[test]
    fn inverse_dated_futures_resolve_their_pair_from_the_base_coin() {
        // `BTC260904` has no separator; the quote is implied to be USD.
        let instruments = parse_inverse(INVERSE_DATED).unwrap();
        let dated = instruments.first().expect("the fixture carries a contract");

        assert_eq!(dated.base, "BTC");
        assert_eq!(dated.quote, "USD");
        assert_eq!(dated.kind, InstrumentKind::Future);
        assert!(dated.contract.as_ref().unwrap().expiry.is_some());
    }

    #[test]
    fn an_error_status_is_a_rejection() {
        let body = br#"{"status":"error","err_msg":"invalid","data":[]}"#;
        assert!(matches!(
            parse_spot(body),
            Err(SourceError::Rejected { .. })
        ));
    }

    #[test]
    fn garbage_is_a_decode_error() {
        assert!(parse_spot(b"<html>nope</html>").is_err());
        assert!(parse_linear(b"<html>nope</html>").is_err());
    }
}
