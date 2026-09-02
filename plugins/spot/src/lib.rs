//! A paper-trading adapter that simulates a **spot exchange account**.
//!
//! A spot account owns **asset balances, not directional exposure**.
//! Buying BTCUSDT does not open a long; it moves USDT into BTC. There is
//! no leverage, no liquidation, no borrowing, no short position and no
//! unrealised profit in the futures sense — the account simply holds more
//! of one asset and less of another, at whatever price it paid.
//!
//! This crate is the third settlement model, and it is the one that tests
//! the kernel's seam hardest: it has no positions at all, no risk state,
//! nothing to force-close and nothing that time costs. If a book with none
//! of those can implement the same trait as one built entirely out of
//! them, the seam is about settlement rather than about margin — which is
//! what it claims to be.
//!
//! Two rules here are the ones a leveraged simulator gets wrong when it is
//! stretched over spot:
//!
//! - **You cannot sell what you do not hold.** A sell with insufficient
//!   base is refused, not shorted.
//! - **The fee comes out of the asset the trade produces** — base on a
//!   buy, quote on a sell — not out of one account currency.

/// Asset balances, with free and locked kept apart.
pub mod balances;
/// The spot account as a `SettlementModel`.
pub mod model;
