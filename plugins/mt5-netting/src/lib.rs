//! A paper-trading adapter that simulates a **MetaTrader 5 netting
//! account**.
//!
//! Netting is a position-accounting rule: an account holds **at most one
//! position per symbol**, and every trade in that symbol is folded into it
//! rather than sitting beside it. It is what MetaTrader calls the netting
//! system, what an exchange account uses, and what a crypto perpetual
//! venue's one-way mode resembles.
//!
//! Every fill after the first is exactly one of four transitions — add,
//! partial reduce, flat, or reversal. There is no fifth: a hedging
//! account's close-by does not exist here, because there is never a second
//! ticket to close against.
//!
//! This crate exists to answer a question about the kernel rather than
//! only to simulate a broker. `senken-sim-core`'s seam claims that a
//! second trading system can be added as one settlement model with **no
//! edit to the kernel**. This is that second system, and it is written
//! that way deliberately: if it had needed to reach into
//! `senken-sim-core`, the seam would have been in the wrong place and
//! cheaper to move now than after the fourth.

/// The netting book and its four transitions.
pub mod book;
/// The netting account as a `SettlementModel`.
pub mod model;
