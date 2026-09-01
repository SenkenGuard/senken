//! The shared tick-to-bar fold used by live alerts.
//!
//! It lives in `senken-subscription`, alongside the tick contract, so chart
//! sessions and alerts cannot diverge over what constitutes a closed bar.

pub use senken_subscription::TickBarBuilder;
