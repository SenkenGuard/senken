//! The [`Indicator`] trait: the shape every indicator in
//! this crate implements.

use senken_series::Bar;

/// A stateful, incremental market indicator.
///
/// Following the reference implementation (Nautilus's own
/// `indicator.rs`), an indicator here is **not** a pure function from a
/// slice of bars to a value. It is a small state machine that consumes one
/// [`Bar`] at a time through [`handle_bar`](Self::handle_bar) and updates
/// its own internal state in place — an [`Ema`](crate::Ema) never rescans
/// the bars behind it, which is exactly what makes updating it on every new
/// live bar affordable. A batch recompute over the whole history on every
/// new bar is the thing this trait shape rules out by construction.
///
/// Backfilling history uses this same method: replay the stored bars one by
/// one through [`handle_bar`](Self::handle_bar), oldest first. There is
/// deliberately no second "compute over a whole series" method anywhere in
/// this crate — a batch path that could disagree with the live path is
/// exactly the bug ruled out here by only ever providing one way in.
///
/// [`initialized`](Self::initialized) is part of the contract, not an
/// afterthought: an indicator's first few values are a warm-up artefact,
/// not a value a consumer should act on (an EMA's first output is not an
/// EMA). A consumer must be able to ask whether enough input has been seen
/// before trusting whatever a concrete indicator's own accessor(s) return —
/// this trait says nothing about what those accessors look like, because
/// they cannot: an [`Ema`](crate::Ema) reports one number, a
/// [`Macd`](crate::Macd) reports three, and forcing both through one
/// `value() -> f64` method on this trait would make the second case a
/// retrofit instead of a design.
pub trait Indicator {
    /// A short, human-readable name for this indicator, e.g. `"Ema"`.
    fn name(&self) -> String;

    /// Whether at least one bar has been handled since construction or the
    /// last [`reset`](Self::reset).
    ///
    /// This is weaker than [`initialized`](Self::initialized): an
    /// indicator can have inputs long before it has *enough* of them.
    fn has_inputs(&self) -> bool;

    /// Whether this indicator has seen enough bars for its value(s) to be
    /// meaningful. `false` during warm-up, even once
    /// [`has_inputs`](Self::has_inputs) is `true`.
    fn initialized(&self) -> bool;

    /// Feeds one new bar into the indicator, updating its internal state.
    ///
    /// Bars must be handed to this in chronological order (oldest first)
    /// for both live updates and backfill replay — this method is the only
    /// way data ever enters an indicator.
    fn handle_bar(&mut self, bar: &Bar);

    /// Returns the indicator to the state it was in immediately after
    /// construction: no inputs, not initialized, every accumulator at its
    /// zero value.
    fn reset(&mut self);
}
