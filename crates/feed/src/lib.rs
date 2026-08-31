//! The live-price feed (Rust half).
//!
//! Implements `senken-subscription`'s
//! [`VenueConnection`](senken_subscription::VenueConnection)/
//! [`VenueConnector`](senken_subscription::VenueConnector) ports against a
//! real venue WebSocket, and delivers last-price updates to whoever holds a
//! [`Lease`](senken_subscription::Lease) — a chart pane, a watchlist row, an
//! alert, a position, or anything else added later. This
//! crate has no idea which of those it is talking to, and is not supposed
//! to: the dial/reconnect engine behind [`WsVenueConnection`] and
//! [`WsVenueConnector`] is entirely generic across venues, and every
//! venue-specific fact (URL, subscribe protocol, message shape) is isolated
//! behind [`VenueProtocol`] and, for OKX specifically, verified live rather
//! than assumed — see [`okx`]'s module docs for exactly what was confirmed
//! and what was not.
//!
//! # Sharing the venue's rate budget
//!
//! A venue's connection limit is an IP-level fact, the same one
//! [`senken_venue::LimitGroup`] already exists to track for REST traffic
//!. A WS dial through [`WsVenueConnector`] draws on the exact
//! same [`LimitGroup`](senken_venue::LimitGroup) a plugin's REST client uses
//! for that venue, via
//! [`senken_venue::LimitGroup::acquire_for_connect`], rather than opening an
//! unbudgeted side channel.
//!
//! # What this crate does not do
//!
//! It never decides *which* instruments to subscribe — that is entirely the
//! pool's job, driven by leases. It never persists a price (the //! scope note) and never aggregates ticks into bars (a derived series is never persisted, and this stage does not even attempt the aggregation). It never reads a wall clock on the price path:
//! every [`senken_subscription::PriceUpdate`] this crate produces carries a
//! timestamp decoded from the venue's own message.

mod connection;
mod connector;
pub mod okx;
mod protocol;
mod symbol_map;

pub use connection::WsVenueConnection;
pub use connector::WsVenueConnector;
pub use protocol::VenueProtocol;
pub use symbol_map::{IdentitySymbolMap, SymbolMap};

// Re-exported so a caller building a `VenueProtocol` or wiring a connector
// needs no direct dependency on `senken-subscription` just to name these.
pub use senken_subscription::{ConnectionError, PriceUpdate};
