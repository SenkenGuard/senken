//! The trend family: [`Sma`], [`Ema`] and [`Wma`], plus the
//! [`MovingAverage`] trait other indicators in this crate compose over.

mod ema;
mod sma;
mod wma;

pub use ema::Ema;
pub use sma::Sma;
pub use wma::Wma;

use crate::indicator::Indicator;

/// A single-valued incremental moving average.
///
/// This is the subset of [`Sma`], [`Ema`] and [`Wma`] that a compound
/// indicator composes over — the same role Nautilus's own `MovingAverage`
/// trait plays for its MACD. [`Macd`](crate::Macd) drives its
/// internal fast/slow/signal averages through
/// [`update_raw`](Self::update_raw) rather than
/// [`handle_bar`](Indicator::handle_bar), because the values it feeds them
///   — a close price, then the MACD line itself — do not come from a `Bar`
/// directly.
pub trait MovingAverage: Indicator {
    /// The current average. Meaningless before
    /// [`initialized`](Indicator::initialized).
    fn value(&self) -> f64;

    /// Feeds one already-extracted value into the average, bypassing
    /// [`handle_bar`](Indicator::handle_bar)'s own price extraction.
    ///
    /// [`handle_bar`](Indicator::handle_bar) itself is implemented in terms
    /// of this: it extracts `bar.close` and calls this method, so the two
    /// never disagree about how an average advances.
    fn update_raw(&mut self, value: f64);
}
