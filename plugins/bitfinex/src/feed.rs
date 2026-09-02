//! Bitfinex's public `trades` channel.
//!
//! # What was confirmed live, 2026-09-02
//!
//! Connected to `wss://api-pub.bitfinex.com/ws/2`, sent
//! `{"event":"subscribe","channel":"trades","symbol":"tBTCUSD"}` and
//! received:
//!
//! ```json
//! {"event":"info","version":2,"serverId":"9b8e7fe5-…","platform":{"status":1}}
//! {"event":"subscribed","channel":"trades","chanId":3317,"symbol":"tBTCUSD","pair":"BTCUSD"}
//! [3317,[[1966954107,1788335061659,0.00183304,77687],[1966954096,1788335051273,-0.01759454,77669]]]
//! [1,"te",[1966969153,1788336976468,0.00012941,77531]]
//! [1,"tu",[1966969153,1788336976468,0.00012941,77531]]
//! [3317,"hb"]
//! ```
//!
//! Bitfinex is the one venue here whose data frames **do not name their
//! instrument**. A trade arrives as a bare array keyed by a numeric
//! `chanId` the venue assigns, and the only place that number is tied to a
//! symbol is the `subscribed` event that preceded it. So this protocol
//! keeps that mapping — see [`BitfinexTradesProtocol::channels`] — which
//! makes it the one stateful decoder in this project.
//!
//! Also read from that capture:
//! - **`t` prefixes a trading pair.** The catalog stores `BTCUSD`; the
//!   subscribe wants `tBTCUSD`. The acknowledgement helpfully echoes both,
//!   and it is the unprefixed `pair` this protocol records, so nothing has
//!   to strip the prefix back off.
//! - **`te` and `tu` are the same trade twice** — identical id, time,
//!   amount and price, sent as "executed" then "updated". Only `te` is
//!   decoded; decoding both would report every trade's volume twice.
//! - **The amount's sign is the side.** `-0.01759454` is a sell of
//!   0.01759454, not a negative quantity, so the magnitude is what becomes
//!   the volume.
//! - Amounts and prices are bare JSON numbers, read through
//!   [`RawValue`](serde_json::value::RawValue) so the venue's digits reach
//!   the scaled-integer parser rather than an `f64`.
//! - `[chanId,"hb"]` is a heartbeat carrying no data.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use senken_marketdata::InstrumentId;
use senken_plugin::live::trade;
use senken_subscription::{ConnectionError, FeedSource, LiveUpdate, SymbolMap, VenueProtocol};
use senken_venue::normalise_symbol;
use serde::Deserialize;
use serde_json::value::RawValue;

/// `wss://api-pub.bitfinex.com/ws/2` — confirmed live 2026-09-02.
pub(crate) const BITFINEX_WS_URL: &str = "wss://api-pub.bitfinex.com/ws/2";

/// Bitfinex prefixes a trading pair with `t`; the catalog stores the pair
/// without it.
const TRADING_PREFIX: char = 't';

/// Bitfinex writes a pair with a `:` when either side needs more than
/// three characters (`TESTBTC:TESTUSD`).
const SEPARATOR: char = ':';

/// Bitfinex's public `trades` channel.
pub(crate) struct BitfinexTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
    /// `chanId` → the pair that channel carries.
    ///
    /// Bitfinex's data frames name no instrument, only this number, and
    /// the number is assigned per connection. An entry is written when the
    /// venue acknowledges a subscribe, which it always does before sending
    /// anything on that channel — so an id reused for a different pair
    /// after a reconnect is overwritten before a frame can be misrouted.
    channels: Mutex<HashMap<i64, String>>,
}

impl BitfinexTradesProtocol {
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
            channels: Mutex::new(HashMap::new()),
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!("no Bitfinex native symbol known for {instrument}"))
        })
    }

    fn instrument(&self, pair: &str) -> Option<InstrumentId> {
        InstrumentId::new(&self.source_id, &normalise_symbol(pair, &[SEPARATOR])).ok()
    }
}

impl VenueProtocol for BitfinexTradesProtocol {
    fn url(&self) -> &str {
        BITFINEX_WS_URL
    }

    fn venue(&self) -> &'static str {
        "bitfinex"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let symbol = self.native_symbol(instrument)?;
        Ok(format!(
            r#"{{"event":"subscribe","channel":"trades","symbol":"{TRADING_PREFIX}{symbol}"}}"#
        ))
    }

    /// Bitfinex unsubscribes by `chanId`, which is only known once the
    /// venue has acknowledged the subscribe.
    ///
    /// # Errors
    /// [`ConnectionError`] when no acknowledgement for this instrument has
    /// been seen yet — there is no channel id to name, and guessing one
    /// would unsubscribe some other instrument's stream.
    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        let symbol = self.native_symbol(instrument)?;
        let channels = self
            .channels
            .lock()
            .map_err(|_| ConnectionError::new("Bitfinex's channel map is poisoned"))?;
        let id = channels
            .iter()
            .find_map(|(id, pair)| (pair == &symbol).then_some(*id))
            .ok_or_else(|| {
                ConnectionError::new(format!(
                    "no Bitfinex channel id known for {instrument}; nothing was subscribed"
                ))
            })?;
        Ok(format!(r#"{{"event":"unsubscribe","chanId":{id}}}"#))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        if let Ok(event) = serde_json::from_str::<Subscribed>(text)
            && event.event == "subscribed"
            && event.channel == "trades"
            && let Ok(mut channels) = self.channels.lock()
        {
            channels.insert(event.chan_id, event.pair);
            return Vec::new();
        }

        let Ok(frame) = serde_json::from_str::<Vec<&RawValue>>(text) else {
            return Vec::new();
        };
        let [channel, rest @ ..] = frame.as_slice() else {
            return Vec::new();
        };
        let Ok(chan_id) = serde_json::from_str::<i64>(channel.get()) else {
            return Vec::new();
        };
        let Some(pair) = self
            .channels
            .lock()
            .ok()
            .and_then(|channels| channels.get(&chan_id).cloned())
        else {
            return Vec::new();
        };
        let Some(instrument) = self.instrument(&pair) else {
            return Vec::new();
        };

        // Two shapes carry trades: `[id, [[..],[..]]]` — the snapshot the
        // venue sends on subscribing — and `[id, "te", [..]]`. The `"tu"`
        // form repeats a `"te"` already delivered and is skipped.
        let rows: Vec<Row<'_>> = match rest {
            [payload] => serde_json::from_str(payload.get()).unwrap_or_default(),
            [tag, payload] => {
                if serde_json::from_str::<&str>(tag.get()).ok() != Some("te") {
                    return Vec::new();
                }
                serde_json::from_str::<Row<'_>>(payload.get())
                    .map(|row| vec![row])
                    .unwrap_or_default()
            }
            _ => return Vec::new(),
        };

        rows.iter()
            .filter_map(|row| {
                let ts = senken_core::UnixNanos::from_millis(row.1)?;
                // The sign is the side; the magnitude is the size.
                let amount = row.2.get().trim_start_matches('-');
                Some((
                    instrument.clone(),
                    LiveUpdate::Price(trade(ts, row.3.get(), amount)?),
                ))
            })
            .collect()
    }
}

/// The acknowledgement that ties a `chanId` to a pair.
#[derive(Debug, Deserialize)]
struct Subscribed {
    event: String,
    #[serde(default)]
    channel: String,
    #[serde(default, rename = "chanId")]
    chan_id: i64,
    /// The pair *without* the `t` prefix — exactly the catalog's form.
    #[serde(default)]
    pair: String,
}

/// One trade: `[id, ms, signed amount, price]`.
type Row<'a> = (i64, i64, &'a RawValue, &'a RawValue);

/// Bitfinex's live-feed registration — spot only. Its perpetual pairs use
/// the same socket under an `f` prefix, which no capture here has used.
pub(crate) struct BitfinexFeedSource {
    source_ids: Vec<String>,
}

impl BitfinexFeedSource {
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl FeedSource for BitfinexFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        false
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(BitfinexTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{BITFINEX_WS_URL, BitfinexTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::{IdentitySymbolMap, LiveUpdate, VenueProtocol};
    use std::sync::Arc;

    /// The captured acknowledgement, verbatim.
    const SUBSCRIBED: &str = r#"{"event":"subscribed","channel":"trades","chanId":3317,"symbol":"tBTCUSD","pair":"BTCUSD"}"#;

    fn protocol() -> BitfinexTradesProtocol {
        BitfinexTradesProtocol::new(crate::SPOT_ID, Arc::new(IdentitySymbolMap))
    }

    fn instrument() -> InstrumentId {
        InstrumentId::new(crate::SPOT_ID, "BTCUSD").unwrap()
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), BITFINEX_WS_URL);
    }

    /// The catalog stores `BTCUSD`; the subscribe wants `tBTCUSD`.
    #[test]
    fn the_subscribe_frame_adds_the_trading_pair_prefix() {
        let frame: serde_json::Value =
            serde_json::from_str(&protocol().subscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["event"], "subscribe");
        assert_eq!(frame["channel"], "trades");
        assert_eq!(frame["symbol"], "tBTCUSD");
    }

    /// Bitfinex data frames name no instrument. Without the mapping the
    /// acknowledgement carries, a trade cannot be attributed at all.
    #[test]
    fn a_trade_before_its_acknowledgement_is_dropped_rather_than_guessed() {
        let frame = r#"[3317,"te",[1966969153,1788336976468,0.00012941,77531]]"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn a_trade_after_its_acknowledgement_decodes_to_the_exact_traded_price() {
        let protocol = protocol();
        assert!(protocol.parse_message(SUBSCRIBED).is_empty());

        let updates =
            protocol.parse_message(r#"[3317,"te",[1966969153,1788336976468,0.00012941,77531]]"#);

        assert_eq!(updates.len(), 1);
        let (id, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a te frame must decode to a price update");
        };
        assert_eq!(id, &instrument());
        assert_eq!(update.price, 77_531);
        assert_eq!(update.price_scale, 0);
        assert_eq!(update.qty, senken_series::Volume::Real(12_941));
        assert_eq!(update.qty_scale, 8);
        assert_eq!(update.ts.as_millis(), 1_788_336_976_468);
    }

    /// `tu` repeats a `te` already delivered — same id, time, amount and
    /// price. Decoding both reports every trade's volume twice.
    #[test]
    fn the_update_echo_of_a_trade_is_not_decoded_a_second_time() {
        let protocol = protocol();
        protocol.parse_message(SUBSCRIBED);
        let executed =
            protocol.parse_message(r#"[3317,"te",[1966969153,1788336976468,0.00012941,77531]]"#);
        let updated =
            protocol.parse_message(r#"[3317,"tu",[1966969153,1788336976468,0.00012941,77531]]"#);
        assert_eq!(executed.len(), 1);
        assert!(updated.is_empty());
    }

    /// A negative amount is a sell, not a negative quantity.
    #[test]
    fn the_snapshot_reads_a_sell_as_a_positive_size() {
        let protocol = protocol();
        protocol.parse_message(SUBSCRIBED);

        let updates = protocol.parse_message(
            "[3317,[[1966954107,1788335061659,0.00183304,77687],[1966954096,1788335051273,-0.01759454,77669]]]",
        );

        assert_eq!(updates.len(), 2);
        let (_, LiveUpdate::Price(sell)) = &updates[1] else {
            panic!("the second snapshot row must decode to a price update");
        };
        assert_eq!(sell.qty, senken_series::Volume::Real(1_759_454));
        assert_eq!(sell.qty_scale, 8);
    }

    /// Unsubscribing names the channel id the venue assigned; there is no
    /// symbol form of the request to fall back on.
    #[test]
    fn an_unsubscribe_names_the_channel_id_from_the_acknowledgement() {
        let protocol = protocol();
        assert!(
            protocol.unsubscribe_frame(&instrument()).is_err(),
            "nothing is subscribed yet, so there is no id to name"
        );
        protocol.parse_message(SUBSCRIBED);
        let frame: serde_json::Value =
            serde_json::from_str(&protocol.unsubscribe_frame(&instrument()).unwrap()).unwrap();
        assert_eq!(frame["event"], "unsubscribe");
        assert_eq!(frame["chanId"], 3317);
    }

    #[test]
    fn the_captured_info_frame_and_heartbeat_yield_nothing() {
        let protocol = protocol();
        protocol.parse_message(SUBSCRIBED);
        assert!(
            protocol
                .parse_message(
                    r#"{"event":"info","version":2,"serverId":"9b8e7fe5","platform":{"status":1}}"#
                )
                .is_empty()
        );
        assert!(protocol.parse_message(r#"[3317,"hb"]"#).is_empty());
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json").is_empty());
        assert!(protocol().parse_message("[]").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }
}
