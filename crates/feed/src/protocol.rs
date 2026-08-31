//! [`VenueProtocol`] — the one seam where a venue's own wire format lives,
//! kept apart from the reconnect/backoff/rate-limiting machinery that is the
//! same for every venue.

use senken_marketdata::InstrumentId;
use senken_subscription::{ConnectionError, PriceUpdate};

/// What one venue's WebSocket protocol looks like.
///
/// No venue's stream cap, message shape or
/// subscribe protocol has been verified" in this project — this trait is
/// where that unverified-until-checked boundary is drawn: the
/// dial/reconnect engine behind [`WsVenueConnection`](crate::WsVenueConnection)
/// and [`WsVenueConnector`](crate::WsVenueConnector) is generic across
/// venues and tested against a fake; everything venue-specific lives behind
/// this trait, in a module that says plainly what it verified live versus
/// what it assumed.
pub trait VenueProtocol: Send + Sync + 'static {
    /// The WebSocket URL to dial.
    fn url(&self) -> &str;

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
    fn parse_message(&self, text: &str) -> Vec<(InstrumentId, PriceUpdate)>;
}
