//! The incremental indicator engine.
//!
//! Every indicator here implements [`Indicator`]: a small, stateful
//! machine that consumes bars one at a time through
//! [`handle_bar`](Indicator::handle_bar) and reports whether it has seen
//! enough of them yet through [`initialized`](Indicator::initialized). Two
//! properties matter more than any single formula in this crate:
//!
//! - **Incremental, not batch.** A new bar updates an indicator's existing
//!   state; it never triggers a recompute over the bars behind it. That is
//!   what makes updating an indicator on every live bar affordable at all.
//! - **One code path, live or backfilled.** Backfilling history means
//!   replaying it bar by bar through the same
//!   [`handle_bar`](Indicator::handle_bar) a live feed calls. There is no
//!   second, batch-shaped way to compute a value — a path that could
//!   disagree with the incremental one is exactly the bug this design
//!   rules out by only ever providing one way in. Every indicator's test
//!   module proves this for that indicator specifically.
//!
//! # The ten built-ins
//!
//! [`Sma`], [`Ema`] and [`Wma`] (trend, overlay); [`Rsi`], [`Macd`] and
//! [`Stochastic`] (momentum, sub-pane); [`BollingerBands`] and [`Atr`]
//! (volatility); [`Vwap`] and [`Volume`] (volume). Three of them — [`Macd`],
//! [`BollingerBands`] and [`Stochastic`] — report more than one value per
//! bar, which is why [`Indicator`] itself has no `value() -> f64` method:
//! forcing every indicator through one number would have made those three
//! a retrofit instead of a design.
//!
//! # `f64` is correct here
//!
//! This project's rule elsewhere is "never `f64`", and it is easy to
//! misapply that rule to this crate. The ban covers **prices, quantities
//! and money** — values that must be exact and must not lose a cent. An
//! indicator's output is different in kind: an EMA, an RSI, a standard
//! deviation are **fractional by nature** and exist to be looked at or
//! compared against a threshold, not settled. They are display and
//! decision values, not money, so `f64` is the correct type for them —
//! forcing them into scaled integers would add rounding error rather than
//! remove it.
//!
//! [`Bar`](senken_series::Bar)'s own fields stay scaled integers; the scale
//! itself lives on the series, not on the bar, so widening a bar's fields
//! into the `f64`s an indicator computes with is this crate's job, done in
//! exactly one place (the private `convert::scaled_to_f64`).
//!
//! The boundary this draws is hard and one-directional: an indicator value
//! may be `f64`, but an order price *derived from* one must be rounded back
//! to the instrument's tick as a scaled integer before it reaches anything
//! that trades. This crate never does that rounding itself — it has no
//! notion of an instrument's tick size — so nothing here ever produces an
//! order price; it only ever produces values for a human or a strategy to
//! read.
//!
//! # Adding an eleventh indicator
//!
//! A new single-valued indicator (say, a Rate of Change) needs one file
//! implementing [`Indicator`] — construction, `handle_bar`, `reset`, a
//! `value()` accessor and the warm-up bookkeeping for `initialized()` — plus
//! one `pub use` line in its family's `mod.rs` and one in this file. A new
//! *compound* indicator built from existing pieces (a Percentage Price
//! Oscillator, which is a [`Macd`]-shaped calculation over percentages
//! instead of raw price differences) is smaller still, since it composes
//! over [`average::MovingAverage::update_raw`] the same way [`Macd`]
//! already does. Every indicator in this crate is under 140 lines including
//! its tests; [`Macd`] itself is under 40 lines of logic because the
//! incremental discipline it needs already lives in [`Ema`].

mod average;
mod convert;
mod indicator;
mod momentum;
mod volatility;
mod volume;

#[cfg(test)]
mod test_support;

pub use crate::average::{Ema, MovingAverage, Sma, Wma};
pub use crate::indicator::Indicator;
pub use crate::momentum::{Macd, Rsi, Stochastic};
pub use crate::volatility::{Atr, BollingerBands};
pub use crate::volume::{Volume, Vwap};
