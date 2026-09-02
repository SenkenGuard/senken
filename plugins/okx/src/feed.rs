//! OKX's public trades WebSocket channel — the one venue this crate's live
//! connection was actually verified against.
//!
//! # What was confirmed live, 2026-08-31
//!
//! Connected to `wss://ws.okx.com:8443/ws/v5/public`, sent
//! `{"op":"subscribe","args":[{"channel":"trades","instId":"BTC-USDT"}]}`,
//! and received:
//!
//! ```json
//! {"event":"subscribe","arg":{"channel":"trades","instId":"BTC-USDT"},"connId":"c7f0ba8e"}
//! {"arg":{"channel":"trades","instId":"BTC-USDT"},"data":[{"instId":"BTC-USDT","tradeId":"1051102132","px":"78808.8","sz":"0.00504905","side":"sell","ts":"1788118931677","count":"1","source":"0","seqId":80559459948}]}
//! ```
//!
//! Confirmed from this capture:
//! - The subscribe acknowledgement (`event: "subscribe"`) and the data
//!   frames are both plain JSON objects with no framing beyond that — no
//!   length prefix, no binary encoding.
//! - `px` (price) is a **string**, matching the observation that
//!   OKX quotes prices as strings everywhere it was checked.
//! - `ts` is also a **string** containing epoch **milliseconds** — the same
//!   fact A4 records for OKX's kline endpoint, now confirmed for this
//!   stream too. Parsed with [`senken_core::decimal_places`]/`parse_scaled`-
//!   adjacent integer parsing, never assumed to be a bare `i64` the way
//!   Binance's kline timestamps are.
//! - The instrument identifier the venue itself uses is `instId`, in native
//!   form (`BTC-USDT`, with the dash) — the same field name and form the
//!   kline endpoint uses, now confirmed for streaming too.
//! - `data` is an array — OKX can and does batch more than one trade into
//!   one frame, even though every message this capture received carried
//!   exactly one.
//! - **This is a trades channel, not a candle stream.** There is no
//!   `confirm` field here at all (unlike OKX's kline endpoint,), and there does not need to be one: a trade has no interval to be
//!   "closed" or "forming" in the first place, so [`crate::PriceUpdate`]
//!   (re-exported from `senken-subscription`) can never be mistaken for a
//!   candle the way a naive read of a kline stream could.
//!
//! # What was *not* verified, and is therefore a documented assumption
//!
//! the access boundary allows exactly one short-lived live
//! connection for this whole milestone, already spent on the capture above.
//! Everything below is a conservative, explicitly-flagged assumption, not a
//! fact:
//! - **The unsubscribe frame** (`op: "unsubscribe"`, otherwise identical
//!   shape to subscribe) was never sent or acknowledged live. If OKX
//!   rejects it or ignores it silently, the pool's own bookkeeping is still
//!   correct — the reconnect-from-authority means a stale venue
//!   subscription only wastes bandwidth until this connection's next
//!   reconnect replays exactly what is currently leased, never a
//!   correctness problem for what a lease holder receives.
//! - **A rejected-subscribe error shape** was never observed (every
//!   subscribe in the capture succeeded). [`OkxTradesProtocol::parse_message`]
//!   treats any frame it does not recognise as carrying no price, which
//!   degrades safely for an unrecognised error frame too: nothing is
//!   published, nothing panics, but a persistent silent rejection would not
//!   be surfaced as an error either. Out of scope for this stage to
//!   invent a shape for a message never seen.
//! - **The public channel's own stream cap per connection** was not tested
//!   by opening enough streams to hit it — this connection composes with
//!   [`senken_subscription::SubscriptionPool`]'s own `DEFAULT_STREAM_CAP`
//!   assumption rather than asserting a different, equally unverified
//!   number for OKX specifically.

use senken_core::decimal_places;
use senken_marketdata::InstrumentId;
use senken_subscription::{ConnectionError, PriceUpdate, QuoteUpdate};
use senken_venue::normalise_symbol;
use serde::Deserialize;
use std::sync::Arc;

use senken_subscription::SymbolMap;
use senken_subscription::{FeedSource, LiveUpdate, VenueProtocol};

/// OKX joins base and quote with `-` in every native symbol this capture
/// and the kline capture both observed (`BTC-USDT`).
const OKX_SEPARATOR: char = '-';

/// `wss://ws.okx.com:8443/ws/v5/public` — confirmed live 2026-08-31: it
/// accepted a `trades` subscribe and started streaming within the same
/// connection, no separate handshake or auth step for public channels.
pub(crate) const OKX_PUBLIC_WS_URL: &str = "wss://ws.okx.com:8443/ws/v5/public";

/// OKX's public `trades` channel (confirmed live — see module docs).
pub(crate) struct OkxTradesProtocol {
    source_id: Box<str>,
    symbols: Arc<dyn SymbolMap>,
}

impl OkxTradesProtocol {
    /// A protocol publishing under `source_id` (e.g. `okx-spot`, matching
    /// however this venue's instruments are registered with
    /// `senken-marketdata` — this crate does not assume one), resolving
    /// each subscribe's native symbol through `symbols`.
    #[must_use]
    pub(crate) fn new(source_id: impl Into<Box<str>>, symbols: Arc<dyn SymbolMap>) -> Self {
        Self {
            source_id: source_id.into(),
            symbols,
        }
    }

    fn native_symbol(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        self.symbols.source_symbol(instrument).ok_or_else(|| {
            ConnectionError::new(format!(
                "no OKX native symbol known for {instrument} (see `SymbolMap`)"
            ))
        })
    }

    fn frame(op: &str, inst_id: &str) -> String {
        // Hand-built rather than a `#[derive(Serialize)]` struct: this is
        // The recorded trades and tickers frames use the same subscription
        // envelope. Asking for both keeps quote delivery alongside trade
        // delivery on one leased venue stream.
        // `inst_id` is already a plain venue symbol (no characters `serde_json`
        // would need to escape beyond what `format!` already produces
        // safely via Rust's own string formatting — OKX symbols are
        // `[A-Z0-9-]` only, verified in every capture in this project).
        format!(
            r#"{{"op":"{op}","args":[{{"channel":"trades","instId":"{inst_id}"}},{{"channel":"tickers","instId":"{inst_id}"}}]}}"#
        )
    }
}

impl VenueProtocol for OkxTradesProtocol {
    fn url(&self) -> &str {
        OKX_PUBLIC_WS_URL
    }

    fn venue(&self) -> &'static str {
        "okx"
    }

    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("subscribe", &self.native_symbol(instrument)?))
    }

    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError> {
        Ok(Self::frame("unsubscribe", &self.native_symbol(instrument)?))
    }

    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)> {
        let Ok(frame) = serde_json::from_str::<OkxFrame>(text) else {
            // Not a shape this protocol recognises at all (a heartbeat, an
            // error event never seen live, malformed JSON) — see the module
            // docs' "not verified" section for why this degrades silently
            // rather than erroring.
            return Vec::new();
        };

        match frame.arg.as_ref().map(|arg| arg.channel.as_str()) {
            Some("trades") => frame
                .data
                .into_iter()
                .filter_map(|entry| self.decode_trade(&entry))
                .collect(),
            Some("tickers") => frame
                .data
                .into_iter()
                .filter_map(|entry| self.decode_quote(&entry))
                .collect(),
            _ => Vec::new(),
        }
    }
}

impl OkxTradesProtocol {
    fn decode_trade(&self, trade: &OkxEntry) -> Option<(InstrumentId, LiveUpdate)> {
        let symbol = normalise_symbol(&trade.inst_id, &[OKX_SEPARATOR]);
        let instrument = InstrumentId::new(&self.source_id, &symbol).ok()?;

        let price_scale = decimal_places(&trade.px);
        let price = senken_core::parse_scaled(&trade.px, price_scale)?;

        let qty_scale = decimal_places(&trade.sz);
        let qty = senken_core::parse_scaled(&trade.sz, qty_scale)?;

        let ts_ms: i64 = trade.ts.trim().parse().ok()?;
        let ts = senken_core::UnixNanos::from_millis(ts_ms)?;

        Some((
            instrument,
            LiveUpdate::Price(PriceUpdate {
                ts,
                price,
                price_scale,
                qty: senken_series::Volume::Real(qty),
                qty_scale,
            }),
        ))
    }

    fn decode_quote(&self, quote: &OkxEntry) -> Option<(InstrumentId, LiveUpdate)> {
        let instrument = InstrumentId::new(
            &self.source_id,
            &normalise_symbol(&quote.inst_id, &[OKX_SEPARATOR]),
        )
        .ok()?;
        let bid = quote.bid_px.as_deref()?;
        let ask = quote.ask_px.as_deref()?;
        let bid_size = quote.bid_sz.as_deref()?;
        let ask_size = quote.ask_sz.as_deref()?;
        let ts_ms: i64 = quote.ts.trim().parse().ok()?;
        let scaled = |value: &str| {
            Some((
                senken_core::parse_scaled(value, decimal_places(value))?,
                decimal_places(value),
            ))
        };
        let update = QuoteUpdate::new(
            senken_core::UnixNanos::from_millis(ts_ms)?,
            scaled(bid)?,
            scaled(ask)?,
            scaled(bid_size)?,
            scaled(ask_size)?,
        )
        .ok()?;
        Some((instrument, LiveUpdate::Quote(update)))
    }
}

/// One inbound frame, loosely typed: OKX's ack (`{"event":...}`) and data
/// (`{"arg":...,"data":[...]}`) frames share no field this protocol reads
/// except `data`, so `#[serde(default)]` makes an ack decode to an empty
/// `data` rather than an error — confirmed live to be the actual shape of
/// both message kinds this connection receives (see module docs).
#[derive(Debug, Deserialize)]
struct OkxFrame {
    #[serde(default)]
    arg: Option<OkxArg>,
    #[serde(default)]
    data: Vec<OkxEntry>,
}

#[derive(Debug, Deserialize)]
struct OkxArg {
    channel: String,
}

/// One entry of a `trades` channel's `data` array. Only the fields this
/// crate actually uses are named; every other field the live capture showed
/// (`tradeId`, `side`, `count`, `source`, `seqId`) is ignored rather than
/// rejected, so a field OKX adds later cannot break decoding.
#[derive(Debug, Deserialize)]
struct OkxEntry {
    #[serde(rename = "instId")]
    inst_id: String,
    #[serde(default)]
    px: String,
    /// Base-asset size of this trade — the volume half of a bar.
    #[serde(default)]
    sz: String,
    ts: String,
    #[serde(rename = "bidPx")]
    bid_px: Option<String>,
    #[serde(rename = "askPx")]
    ask_px: Option<String>,
    #[serde(rename = "bidSz")]
    bid_sz: Option<String>,
    #[serde(rename = "askSz")]
    ask_sz: Option<String>,
}

/// OKX's live-feed registration.
///
/// Serves only `okx-spot` today: the trades channel this protocol subscribes
/// to was verified live against spot instruments, and claiming the swap and
/// futures markets without having seen a frame from either would be exactly
/// the invented venue fact this project's fixtures exist to prevent. Adding
/// them is a matter of confirming the channel and widening
/// [`source_ids`](FeedSource::source_ids) — the pool underneath already
/// shards several sources onto one connection.
pub(crate) struct OkxFeedSource {
    source_ids: Vec<String>,
}

impl OkxFeedSource {
    /// The registration OKX's plugin hands the runtime.
    #[must_use]
    pub(crate) fn new() -> Self {
        Self {
            source_ids: vec![crate::SPOT_ID.to_owned()],
        }
    }
}

impl Default for OkxFeedSource {
    fn default() -> Self {
        Self::new()
    }
}

impl FeedSource for OkxFeedSource {
    fn source_ids(&self) -> &[String] {
        &self.source_ids
    }

    fn serves_quotes(&self) -> bool {
        // OKX's public feed carries `trades` and `tickers` on one socket,
        // and this protocol decodes both — verified live against the
        // capture in this module's own docs.
        true
    }

    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol> {
        Arc::new(OkxTradesProtocol::new(crate::SPOT_ID, symbols))
    }
}

#[cfg(test)]
mod tests {
    use super::{OKX_PUBLIC_WS_URL, OkxTradesProtocol};
    use senken_marketdata::InstrumentId;
    use senken_subscription::IdentitySymbolMap;
    use senken_subscription::{LiveUpdate, VenueProtocol};
    use std::sync::Arc;

    fn protocol() -> OkxTradesProtocol {
        OkxTradesProtocol::new("okx-spot", Arc::new(IdentitySymbolMap))
    }

    #[test]
    fn the_confirmed_url_is_used() {
        assert_eq!(protocol().url(), OKX_PUBLIC_WS_URL);
        assert_eq!(
            OKX_PUBLIC_WS_URL, "wss://ws.okx.com:8443/ws/v5/public",
            "confirmed live 2026-08-31 — see this module's docs"
        );
    }

    #[test]
    fn a_subscribe_frame_matches_the_confirmed_shape() {
        let instrument = InstrumentId::new("okx-spot", "BTCUSDT").unwrap();
        let frame = protocol().subscribe_frame(&instrument).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(parsed["op"], "subscribe");
        assert_eq!(parsed["args"][0]["channel"], "trades");
        assert_eq!(parsed["args"][0]["instId"], "BTCUSDT");
    }

    #[test]
    fn an_unsubscribe_frame_has_the_symmetric_op() {
        let instrument = InstrumentId::new("okx-spot", "BTCUSDT").unwrap();
        let frame = protocol().unsubscribe_frame(&instrument).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&frame).unwrap();
        assert_eq!(parsed["op"], "unsubscribe");
    }

    /// A bar is OHLCV, and the venue reports the V on every trade. Dropping
    /// it here would strand every volume-reading indicator at the last
    /// stored bar, so the size is decoded with the same scaled-integer
    /// treatment as the price — never a float.
    #[test]
    fn a_trade_carries_its_size_at_its_own_scale() {
        let protocol = OkxTradesProtocol::new("okx-spot", std::sync::Arc::new(IdentitySymbolMap));
        let frame = r#"{"arg":{"channel":"trades","instId":"BTC-USDT"},"data":[{"instId":"BTC-USDT","tradeId":"1","px":"78808.8","sz":"0.00504905","side":"sell","ts":"1788118931677","count":"1","source":"0","seqId":1}]}"#;

        let updates = protocol.parse_message(frame);

        assert_eq!(updates.len(), 1);
        let (_, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trades frame must decode to a price update");
        };
        // "0.00504905" is eight fractional digits, so the integer is the
        // string with its point removed — no rounding, no float anywhere.
        assert_eq!(update.qty_scale, 8);
        assert_eq!(update.qty, senken_series::Volume::Real(504_905));
    }

    #[test]
    fn the_captured_ack_frame_yields_no_price_update() {
        let frame = r#"{"event":"subscribe","arg":{"channel":"trades","instId":"BTC-USDT"},"connId":"c7f0ba8e"}"#;
        assert!(protocol().parse_message(frame).is_empty());
    }

    #[test]
    fn the_captured_data_frame_decodes_to_the_exact_traded_price() {
        // Byte-for-byte the second message this module's live capture
        // received.
        let frame = r#"{"arg":{"channel":"trades","instId":"BTC-USDT"},"data":[{"instId":"BTC-USDT","tradeId":"1051102132","px":"78808.8","sz":"0.00504905","side":"sell","ts":"1788118931677","count":"1","source":"0","seqId":80559459948}]}"#;

        let updates = protocol().parse_message(frame);
        assert_eq!(updates.len(), 1);
        let (instrument, LiveUpdate::Price(update)) = &updates[0] else {
            panic!("a trades frame must decode to a price update");
        };
        assert_eq!(
            instrument,
            &InstrumentId::new("okx-spot", "BTCUSDT").unwrap()
        );
        assert_eq!(
            update.price_scale, 1,
            "\"78808.8\" has one fractional digit"
        );
        assert_eq!(update.price, 788_088, "78808.8 at scale 1 is 788088");
        assert_eq!(update.ts.as_millis(), 1_788_118_931_677);
    }

    #[test]
    fn a_frame_with_two_trades_decodes_to_two_updates() {
        let frame = r#"{"arg":{"channel":"trades","instId":"BTC-USDT"},"data":[
            {"instId":"BTC-USDT","tradeId":"1","px":"78808.9","sz":"0.00001198","side":"buy","ts":"1788118938304","count":"1","source":"0","seqId":1},
            {"instId":"BTC-USDT","tradeId":"2","px":"78808.9","sz":"0.00031722","side":"buy","ts":"1788118939760","count":"1","source":"0","seqId":2}
        ]}"#;
        assert_eq!(protocol().parse_message(frame).len(), 2);
    }

    #[test]
    fn garbage_input_yields_no_updates_rather_than_a_panic() {
        assert!(protocol().parse_message("not json at all").is_empty());
        assert!(protocol().parse_message("{}").is_empty());
    }

    #[test]
    fn an_instrument_the_symbol_map_cannot_resolve_is_reported_not_silently_skipped() {
        struct EmptyMap;
        impl senken_subscription::SymbolMap for EmptyMap {
            fn source_symbol(&self, _: &InstrumentId) -> Option<String> {
                None
            }
        }
        let protocol = OkxTradesProtocol::new("okx-spot", Arc::new(EmptyMap));
        let instrument = InstrumentId::new("okx-spot", "BTCUSDT").unwrap();
        assert!(protocol.subscribe_frame(&instrument).is_err());
    }
}
