//! [`VenueProtocol`] — the one seam where a venue's own wire format lives,
//! kept apart from the reconnect/backoff/rate-limiting machinery that is the
//! same for every venue.

use crate::connection::ConnectionError;
use crate::price::PriceUpdate;
use crate::quote::QuoteUpdate;
use crate::symbol_map::SymbolMap;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;

use senken_marketdata::InstrumentId;

/// A decoded live market-data message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LiveUpdate {
    /// A last-trade update.
    Price(PriceUpdate),
    /// A best bid and offer update.
    Quote(QuoteUpdate),
}

/// What one venue's WebSocket protocol looks like.
///
/// No venue's stream cap, message shape or
/// subscribe protocol has been verified" in this project — this trait is
/// where that unverified-until-checked boundary is drawn: the
/// dial/reconnect engine behind [`WsVenueConnection`](https://docs.rs/senken-feed)
/// and `senken_feed::WsVenueConnector` is generic across
/// venues and tested against a fake; everything venue-specific lives behind
/// this trait, in a module that says plainly what it verified live versus
/// what it assumed.
#[async_trait]
pub trait VenueProtocol: Send + Sync + 'static {
    /// The WebSocket URL to dial.
    fn url(&self) -> &str;

    /// The URL to dial for *this* attempt, resolved fresh each time.
    ///
    /// Defaults to [`url`](Self::url), which is what every venue whose
    /// endpoint is a constant wants. KuCoin's is not: it hands out a
    /// short-lived token over HTTP that the WebSocket URL has to carry, so
    /// a URL captured once at startup stops working — the token has to be
    /// fetched again for every dial, including every reconnect.
    ///
    /// # Errors
    /// [`ConnectionError`] if the endpoint could not be resolved. The dial
    /// then fails and is retried like any other connect failure.
    async fn endpoint(&self) -> Result<String, ConnectionError> {
        Ok(self.url().to_owned())
    }

    /// A short name for this venue, used only in logs and errors.
    fn venue(&self) -> &str;

    /// Builds the text frame that subscribes `instrument`.
    ///
    /// # Errors
    /// [`ConnectionError`] if `instrument` cannot be translated to this
    /// venue's own wire symbol (see [`crate::SymbolMap`]).
    fn subscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError>;

    /// Builds the text frame that unsubscribes `instrument`.
    ///
    /// # Errors
    /// As [`subscribe_frame`](Self::subscribe_frame).
    fn unsubscribe_frame(&self, instrument: &InstrumentId) -> Result<String, ConnectionError>;

    /// Decodes one inbound text frame into zero or more price updates, each
    /// paired with the normalised [`InstrumentId`] it belongs to.
    ///
    /// Returns an empty `Vec` for a frame that carries no price at all — an
    /// acknowledgement, a heartbeat, an error event — rather than an `Err`:
    /// a frame this protocol does not recognise is not this connection's
    /// failure, only a message it has nothing to publish from.
    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, LiveUpdate)>;

    /// Turns one inbound *binary* frame into the text
    /// [`parse_message`](Self::parse_message) reads, or `None` for a frame
    /// this protocol has nothing to do with.
    ///
    /// Defaults to `None`, which is correct for every venue that only ever
    /// sends text. Two shapes in this project need it and neither is
    /// optional: HTX and BingX compress every frame with gzip, and Upbit
    /// sends plain UTF-8 JSON in a binary frame rather than a text one. A
    /// connection that ignores binary frames receives *nothing at all*
    /// from those three — silently, since a dropped frame is not an error.
    fn decode_binary(&self, _bytes: &[u8]) -> Option<String> {
        None
    }

    /// The frame this protocol must send back after receiving `text`, if
    /// any.
    ///
    /// This is for venue-initiated keep-alives carried as ordinary
    /// application messages rather than WebSocket control frames, which
    /// the transport answers by itself. HTX sends `{"ping":<ts>}` and
    /// closes a connection that does not answer with the matching
    /// `{"pong":<ts>}`; Crypto.com sends `public/heartbeat` and wants
    /// `public/respond-heartbeat` carrying the same id.
    fn reply_to(&self, _text: &str) -> Option<String> {
        None
    }

    /// A frame this protocol must send unprompted, and how often.
    ///
    /// Distinct from [`reply_to`](Self::reply_to): nothing arrives to
    /// trigger it, so an idle socket that never sends it is dropped by the
    /// venue. Bybit and Bitget both work this way.
    fn keepalive(&self) -> Option<(Duration, String)> {
        None
    }
}

/// What a plugin registers to declare that it can stream live prices for
/// one or more of its sources.
///
/// # Why this is a factory and the other capabilities are not
///
/// Instruments, bars and depth are all registered as a finished
/// `Arc<dyn Trait>`: a plugin can build one during activation because
/// everything they need is a client and a URL. A [`VenueProtocol`] cannot
/// be built then. It needs a [`SymbolMap`] to turn Senken's normalised
/// `BTCUSDT` back into whatever the venue's subscribe frame expects
/// (`BTC-USDT` for OKX), and that map is derived from the instrument
/// catalog — which does not exist yet, because assembling it is precisely
/// what every plugin is still in the middle of registering sources for.
///
/// So the plugin hands over the means to build a protocol, and the runtime
/// calls it once it holds a catalog. That ordering is not an inconvenience
/// to work around: it is the runtime deciding when live data starts, which
/// is where that decision belongs.
pub trait FeedSource: Send + Sync {
    /// Every source id this feed serves.
    ///
    /// More than one because a venue's physical stream usually is not
    /// split the way its markets are: OKX's public trades socket carries
    /// spot, swap and futures alike, and all three should share one pool
    /// rather than open a connection each.
    fn source_ids(&self) -> &[String];

    /// Whether this feed carries a best bid and offer, not only last
    /// trades.
    ///
    /// Declared rather than inferred from the pool, because the two are
    /// genuinely different capabilities and a client acts on the
    /// difference: a chart draws bid/ask lines only for a source that has
    /// quotes, and drawing them for one that only streams trades would be a
    /// control that silently does nothing. OKX happens to carry both on one
    /// channel, which is exactly what made "has a pool" look like an
    /// adequate answer while there was only one venue to ask.
    fn serves_quotes(&self) -> bool;

    /// Builds this venue's protocol against a catalog-backed `symbols`.
    ///
    /// Called once per server, after the instrument catalog is available.
    /// The venue name the resulting pool is built with comes from
    /// [`VenueProtocol::venue`], so an implementation does not state it
    /// twice and the two cannot disagree.
    fn protocol(&self, symbols: Arc<dyn SymbolMap>) -> Arc<dyn VenueProtocol>;
}

impl std::fmt::Debug for dyn FeedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FeedSource")
            .field("source_ids", &self.source_ids())
            .field("serves_quotes", &self.serves_quotes())
            .finish()
    }
}
