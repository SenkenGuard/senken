//! The volatility family: [`BollingerBands`] (an overlay) and
//! [`Atr`] (typically sub-pane, though it plots fine as either).

mod atr;
mod bollinger;

pub use atr::Atr;
pub use bollinger::BollingerBands;
