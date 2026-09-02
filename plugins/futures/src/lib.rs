//! A paper-trading adapter that simulates a **crypto perpetual futures
//! account** — Binance USDⓈ-M and Bitget USDT-M, closely enough to share
//! one shape.
//!
//! Three things make this system itself rather than a leveraged spot
//! account:
//!
//! - **Liquidation.** Equity falling below maintenance margin closes the
//!   position, and maintenance margin steps up with notional from a
//!   per-symbol bracket table the venue publishes.
//! - **Funding.** A periodic transfer between longs and shorts, not a fee
//!   to the exchange, settled straight against the balance with no order
//!   and no fill.
//! - **Position and margin mode.** One-way or hedge, isolated or cross,
//!   and the choice changes what liquidation even means.
//!
//! The bracket table is **supplied, never invented**. An account that has
//! not been given one reports no liquidation price at all, because a
//! trader shown a liquidation price believes it and a wrong one is worse
//! than an absent one.

/// The leverage bracket table and the maintenance margin it implies.
pub mod bracket;
/// Funding: a transfer between longs and shorts.
pub mod funding;
/// The futures account as a `SettlementModel`.
pub mod model;
