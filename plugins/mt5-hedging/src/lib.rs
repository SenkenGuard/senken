//! A paper-trading adapter that simulates a **MetaTrader 5 hedging
//! account**.
//!
//! A hedging account is the mode almost every retail forex, gold and CFD
//! broker gives its MT5 clients, and it is the one MetaTrader 4 always
//! used: a buy and a sell on the same symbol coexist as two separate
//! positions rather than netting into one. A trader "locks" a pair long
//! and short, or holds two independent tickets on gold with different
//! stops, and the account's margin rules charge for that possibility.
//!
//! What makes this adapter that account rather than a plausible average
//! of three trading systems:
//!
//! - **Every deal opens its own ticket.** Positions are a list, not a map
//!   keyed by instrument, because the same symbol legitimately holds
//!   several at once and one of them can be long while another is short.
//! - **Margin is per symbol**, by `SYMBOL_TRADE_CALC_MODE`'s own formula
//!   — forex, CFD, CFD-with-leverage or futures — not one blanket
//!   `notional / leverage`.
//! - **Margin call and stop out are different events.** A margin call
//!   blocks opening and closes nothing. A stop out closes the biggest
//!   losing position, then looks again, and repeats until the margin level
//!   recovers.
//!
//! Every broker-set number this needs — the two thresholds, the contract
//! size, the margin percentage, the swap rates — is account settings read
//! from the symbol specification, never a constant invented here. MT5
//! fixes the formulas; brokers fix the numbers.

/// MT5's margin formulas and the four figures a terminal shows.
pub mod margin;
/// The hedging book: one ticket per deal.
pub mod ticket;
