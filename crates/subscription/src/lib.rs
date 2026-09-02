//! The live-data subscription pool.
//!
//! Nothing in Senken polls a venue for a live price. Anything that needs one
//!   — a chart pane, a watchlist row, an alert, an open position — calls
//! [`SubscriptionPool::lease`] for the `(source, symbol)` it cares about and
//! gets back a [`Lease`]: a `Drop` guard, not a handle with an `unsubscribe`
//! method. The pool reference-counts leases per instrument, subscribes to
//! the venue on the first one and unsubscribes on the last, and shards
//! across connections once a venue's configured stream cap is reached. It
//! knows nothing about charts, watchlists, alerts or positions — only about
//! instruments, leases and [`PriceUpdate`]s.
//!
//! # Receiving updates
//!
//! [`Lease::updates`] is the entire consumer-facing contract R3 adds:
//! whatever holds a lease calls it to get a `watch::Receiver` for that
//! instrument's latest price, no separate registration step. A real
//! [`VenueConnection`] calls [`SubscriptionPool::publish`] every time it
//! decodes one off the wire; this crate has no idea, and does not need one,
//! whether the caller on either end is a chart, a watchlist row or an alert.
//!
//! # Why `Drop`, not a manual `unsubscribe`
//!
//! A manual release relies on every caller remembering to call it. A pane
//! that closes without releasing leaks its subscription silently — the
//! leak stays invisible until a venue's connection cap is hit and unrelated
//! panes start failing to open. Making release a `Drop` effect makes that
//! leak unrepresentable: there is no code path through which a caller can
//! hold a [`Lease`] and *not* eventually release it, short of leaking the
//! guard itself (`Box::leak`, a cycle, `mem::forget`) — the ordinary kind of
//! leak every `Drop`-based resource in Rust already accepts as out of scope.
//!
//! # Async release from a non-async `Drop`
//!
//! Releasing a lease can require an async unsubscribe call to the venue, and
//! [`Drop::drop`] cannot `.await` anything. This crate resolves that with an
//! actor: [`SubscriptionPool::new`] spawns one task that owns all of the
//! pool's mutable state and is the only thing that ever talks to a
//! [`VenueConnection`]. [`Lease::drop`] does not perform the unsubscribe
//! itself — it only sends a message on an unbounded channel, which is a
//! synchronous, non-blocking, infallible-from-`Drop`'s-perspective
//! operation regardless of whether the guard is dropped inside an async
//! task, a blocking thread, or a panic unwind. The actor performs the actual
//! `await` later, serialised against every other lease/release/reconnect so
//! none of them can race each other or need a lock. [`SubscriptionPool::flush`]
//! exists for callers (chiefly tests) that must observe a release's effect
//! before asserting on it, since the effect is now asynchronous relative to
//! the `drop` call that triggered it.
//!
//! # Venue stream caps
//!
//! A venue caps how many streams one connection may carry. This crate never
//! discovers that cap from an error — doing so
//! at the moment a user opens a pane is the wrong time to find out — so the
//! cap is configuration, passed to [`SubscriptionPool::with_cap`] (or its
//! conservative, explicitly-not-a-verified-fact default via
//! [`SubscriptionPool::new`]). Once every existing connection ("shard") is
//! at the cap, the pool opens another one through the [`VenueConnector`] it
//! was built with, rather than failing the lease.

mod book_session;
mod connection;
mod indicator_session;
mod pool;
mod price;
mod protocol;
mod quote;
mod session;
mod symbol_map;

// The book port itself lives in `senken-marketdata`, beside
// `MarketDataSource`: it needs only a `SourceSymbol` and a `SourceError`,
// so putting it here would have made every venue plugin compile this
// crate's pool, its tokio runtime and its indicators just to declare that
// it can serve depth. Re-exported so this crate's own consumers — the live
// book session below, and `senken-feed`'s adapters — still need one import.
pub use book_session::{
    BookSessionHandle, BookSessionRegistry, BookState, DEFAULT_REFRESH_INTERVAL,
};
pub use connection::{ConnectionError, VenueConnection, VenueConnector};
pub use indicator_session::{
    IndicatorEngine, IndicatorReading, IndicatorSessionHandle, IndicatorSessionKey,
    IndicatorSessionRegistry,
};
pub use pool::{Lease, PoolError, SubscriptionPool};
pub use price::PriceUpdate;
pub use protocol::{FeedSource, LiveUpdate, VenueProtocol};
pub use quote::{QuoteError, QuoteLease, QuoteSource, QuoteUpdate};
pub use senken_marketdata::book::{BookError, BookLevel, BookSnapshot, BookSource};
pub use session::TickBarBuilder;
pub use symbol_map::{IdentitySymbolMap, SymbolMap};
