//! The WebSocket dial/reconnect engine for live market data.
//!
//! Implements `senken-subscription`'s
//! [`VenueConnection`](senken_subscription::VenueConnection)/
//! [`VenueConnector`](senken_subscription::VenueConnector) ports against a
//! real venue WebSocket, and delivers last-price updates to whoever holds a
//! [`Lease`](senken_subscription::Lease) — a chart pane, a watchlist row, an
//! alert, a position, or anything else added later. This crate has no idea
//! which of those it is talking to, and is not supposed to.
//!
//! **No venue lives here.** The engine behind [`WsVenueConnection`] and
//! [`WsVenueConnector`] is entirely generic, and every venue-specific fact
//! (URL, subscribe protocol, message shape) sits behind
//! [`VenueProtocol`] in that venue's own plugin, next to the recorded
//! response it was verified against. Anything else and a venue's quirks
//! leak into machinery twenty-one other venues share.
//!

mod connection;
mod connector;
mod proxy;

pub use connection::WsVenueConnection;
pub use connector::WsVenueConnector;

// Re-exported so a caller building a `VenueProtocol` or wiring a connector
// needs no direct dependency on `senken-subscription` just to name these.
// The live-feed *ports* live beside their siblings (`BookSource`,
// `QuoteSource`, `VenueConnector`) in `senken-subscription`; this crate holds
// the WebSocket engine and the venue implementations that satisfy them.
// Re-exported so a venue adapter needs one import, not two.
pub use senken_subscription::{
    ConnectionError, IdentitySymbolMap, LiveUpdate, PriceUpdate, QuoteUpdate, SymbolMap,
    VenueProtocol,
};
