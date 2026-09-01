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

    /// Clones this indicator's current state for a provisional calculation.
    ///
    /// The returned indicator is independent: advancing it must never alter
    /// the confirmed state held by the caller.
    fn snapshot(&self) -> Box<dyn Indicator>;

    /// Returns the indicator to the state it was in immediately after
    /// construction: no inputs, not initialized, every accumulator at its
    /// zero value.
    fn reset(&mut self);
}

#[cfg(test)]
mod tests {
    use crate::average::MovingAverage;
    use crate::indicator::Indicator;
    use crate::test_support::bar;
    use crate::{Atr, BollingerBands, Ema, Macd, Rsi, Sma, Stochastic, Volume, Vwap, Wma};
    use senken_series::Bar;

    /// Bars deliberately vary on every OHLCV field so that any
    /// order-sensitive built-in computes a different final reading when
    /// fed in reverse.
    fn bars() -> Vec<Bar> {
        vec![
            bar(100, 105, 95, 100, 10),
            bar(101, 110, 98, 108, 20),
            bar(108, 112, 100, 95, 5),
            bar(95, 100, 90, 98, 15),
            bar(98, 120, 96, 115, 30),
            bar(115, 118, 108, 110, 8),
        ]
    }

    /// A tolerance well above ordinary floating-point rounding but far
    /// below any real difference a reversed feed order would produce.
    fn differs(a: f64, b: f64) -> bool {
        (a - b).abs() > 1e-6
    }

    fn feed<I: Indicator>(mut indicator: I, bars: impl Iterator<Item = Bar>) -> I {
        for bar in bars {
            indicator.handle_bar(&bar);
        }
        indicator
    }

    /// This crate's whole incremental design rests on
    /// [`Indicator::handle_bar`]'s own contract: bars arrive oldest first.
    /// This proves that contract is load-bearing rather than merely
    /// documented — feeding the same bars in reverse must produce a
    /// different confirmed reading for every built-in whose formula
    /// depends on order, and the property [`Indicator::snapshot`] must
    /// never weaken it (a snapshot that shared state instead of copying it
    /// would still fail this the same way [`crate::dynamic`]'s own
    /// snapshot proof does).
    ///
    /// Exactly one of the ten is exempt: [`Vwap`] is a ratio of two running
    /// sums since construction, and addition is commutative — feeding the
    /// same bars in either order accumulates the same two sums, so it is
    /// correctly order-*insensitive*, not broken.
    #[test]
    fn nine_of_ten_built_ins_break_when_fed_bars_out_of_order() {
        let forward = bars();
        let reversed: Vec<Bar> = forward.iter().rev().copied().collect();

        let sma_fwd = feed(Sma::new(3), forward.iter().copied());
        let sma_rev = feed(Sma::new(3), reversed.iter().copied());
        assert!(differs(sma_fwd.value(), sma_rev.value()), "Sma");

        let ema_fwd = feed(Ema::new(3), forward.iter().copied());
        let ema_rev = feed(Ema::new(3), reversed.iter().copied());
        assert!(differs(ema_fwd.value(), ema_rev.value()), "Ema");

        let wma_fwd = feed(Wma::new(3), forward.iter().copied());
        let wma_rev = feed(Wma::new(3), reversed.iter().copied());
        assert!(differs(wma_fwd.value(), wma_rev.value()), "Wma");

        let rsi_fwd = feed(Rsi::new(3), forward.iter().copied());
        let rsi_rev = feed(Rsi::new(3), reversed.iter().copied());
        assert!(differs(rsi_fwd.value(), rsi_rev.value()), "Rsi");

        let atr_fwd = feed(Atr::new(3), forward.iter().copied());
        let atr_rev = feed(Atr::new(3), reversed.iter().copied());
        assert!(differs(atr_fwd.value(), atr_rev.value()), "Atr");

        let macd_fwd = feed(Macd::new(2, 3, 2), forward.iter().copied());
        let macd_rev = feed(Macd::new(2, 3, 2), reversed.iter().copied());
        assert!(differs(macd_fwd.macd(), macd_rev.macd()), "Macd");

        let stoch_fwd = feed(Stochastic::new(3, 2), forward.iter().copied());
        let stoch_rev = feed(Stochastic::new(3, 2), reversed.iter().copied());
        assert!(differs(stoch_fwd.k(), stoch_rev.k()), "Stochastic");

        let bb_fwd = feed(BollingerBands::new(3, 2.0), forward.iter().copied());
        let bb_rev = feed(BollingerBands::new(3, 2.0), reversed.iter().copied());
        assert!(differs(bb_fwd.upper(), bb_rev.upper()), "BollingerBands");

        let vol_fwd = feed(Volume::new(), forward.iter().copied());
        let vol_rev = feed(Volume::new(), reversed.iter().copied());
        assert!(
            differs(vol_fwd.value(), vol_rev.value()),
            "Volume reports the most recent bar's own volume, so which bar \
             was fed last (order-dependent) must change its reading"
        );

        // The one exemption, proven rather than assumed: same bars, same
        // two running sums, same ratio, regardless of order.
        let vwap_fwd = feed(Vwap::new(), forward.iter().copied());
        let vwap_rev = feed(Vwap::new(), reversed.iter().copied());
        assert!(
            !differs(vwap_fwd.value(), vwap_rev.value()),
            "Vwap's cumulative sums are commutative — it must NOT break"
        );
    }
}
