//! The shared simulation kernel Senken's paper-trading adapters are built
//! on.
//!
//! Four simulated trading systems — an MT5 hedging account, an MT5 netting
//! account, a crypto perpetual futures account and a spot account —
//! disagree about exactly one thing: what a fill does to the account's
//! state. Order intake, resting orders, fill pricing, fee arithmetic and
//! history are identical across all four.
//!
//! So those live here, written once, and the difference is a
//! [`SettlementModel`]. The alternative — four adapters carrying their own
//! copy of the same fifteen hundred lines — does not stay four copies. It
//! drifts, and a fee rounding fixed in one and not the others is a bug
//! nobody can see.
//!
//! This is a crate rather than a module inside one plugin so that a
//! simulator for a system Senken does not ship — a different broker's
//! rules, an exchange nobody here has an account with — can be written
//! outside this repository without vendoring any of it.

/// Fixed-point arithmetic for simulated books.
pub mod money;
/// Fill pricing and resting-order triggers.
pub mod pricing;
/// Account risk, and what a system does when it is breached.
pub mod risk;
/// The one thing four simulated systems genuinely disagree about.
pub mod settlement;

pub use crate::money::{
    BPS_DIVISOR, CASH_SCALE, basis_points, notional, rescale, slip, weighted_average,
};
pub use crate::pricing::{Terms, apply_amendment, is_triggered, market_fill_price};
pub use crate::risk::{ForcedClose, RiskBreach, RiskState};
pub use crate::settlement::{FillContext, Settled, SettlementModel};
