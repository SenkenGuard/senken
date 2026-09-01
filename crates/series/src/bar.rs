//! [`Bar`], [`SeriesKey`] and [`Trade`] — the data itself.

use senken_core::UnixNanos;
use serde::{Deserialize, Serialize};

use crate::spec::{BarSpec, Origin};

/// What market value supplied a bar's OHLC prices.
///
/// This is part of series identity: trade-built and bid-built bars can have
/// different prices even when every other key field is equal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BarPriceBasis {
    /// Prices came from executed trades.
    Trade,
    /// Prices came from the best bid.
    Bid,
}

/// The volume carried by a bar, with its unit made explicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Volume {
    /// Base-asset quantity actually traded, at the series' quantity scale.
    Real(i64),
    /// Number of price changes in the interval, not an asset quantity.
    Tick(u32),
    /// The source did not report volume.
    Absent,
}

impl Volume {
    /// Returns real traded quantity, rejecting other volume units.
    #[must_use]
    pub const fn real(self) -> Option<i64> {
        match self {
            Self::Real(value) => Some(value),
            Self::Tick(_) | Self::Absent => None,
        }
    }

    /// Returns the tick count, rejecting other volume units.
    #[must_use]
    pub const fn ticks(self) -> Option<u32> {
        match self {
            Self::Tick(value) => Some(value),
            Self::Real(_) | Self::Absent => None,
        }
    }
}

/// Identifies one series: the same symbol at the same spec from the same
/// source can still be two different series depending on [`Origin`]
///   — see that type's docs for why merging them would be wrong.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SeriesKey {
    /// The plugin's unit of registration, e.g. `binance-usdm`. An order is
    /// placed at a venue, not merely on a symbol, so this stays part of the
    /// key even though it costs a small amount of denormalisation.
    pub source_id: Box<str>,
    /// The normalised symbol, e.g. `BTCUSDT`.
    pub symbol: Box<str>,
    /// Venue-supplied or locally aggregated.
    pub origin: Origin,
    /// The market value used to build OHLC prices.
    pub price_basis: BarPriceBasis,
    /// The timeframe.
    pub spec: BarSpec,
}

impl SeriesKey {
    /// A key from owned or borrowed string-like inputs.
    #[must_use]
    pub fn new(
        source_id: impl Into<Box<str>>,
        symbol: impl Into<Box<str>>,
        origin: Origin,
        spec: BarSpec,
    ) -> Self {
        Self {
            source_id: source_id.into(),
            symbol: symbol.into(),
            origin,
            price_basis: BarPriceBasis::Trade,
            spec,
        }
    }
}

/// One OHLCV bar.
///
/// Prices and volumes are plain scaled integers, not a [`Scaled`]-style
/// newtype: the scale itself lives in the *series*' file metadata, not
/// on every bar, so a newtype here would carry no scale of its own and
/// would only be unwrapped at every batch boundary for no benefit (see the
/// "no `Price`/`Qty` newtypes" addendum to the design record). Never `f64` —
/// same discipline the instrument layer already enforces.
///
/// [`Scaled`]: senken_core::Scaled
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bar {
    /// The start of the interval this bar covers, exactly aligned to its
    /// spec. Venues disagree about which edge they report (Binance gives
    /// open time, others close); normalising to open time is the plugin's
    /// job, not this crate's.
    pub ts_open: UnixNanos,
    /// The first trade price in the interval, at the series' price scale.
    pub open: i64,
    /// The highest trade price in the interval.
    pub high: i64,
    /// The lowest trade price in the interval.
    pub low: i64,
    /// The last trade price in the interval.
    pub close: i64,
    /// Base-asset volume traded in the interval, at the series' quantity
    /// scale.
    pub volume: Volume,
    /// Quote-asset volume, when the venue reports it.
    pub quote_volume: Option<i64>,
    /// Number of trades in the interval, when the venue reports it.
    pub trade_count: Option<u32>,
    /// Base-asset volume bought by takers, when the venue reports it.
    pub taker_buy_volume: Option<i64>,
}

/// Which side of the book a [`Trade`] executed against — the taker's side.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Side {
    /// The taker bought (hit an ask).
    Buy,
    /// The taker sold (hit a bid).
    Sell,
}

/// A single market event: one trade. Has no duration, so it is never a
/// member of [`BarUnit`](crate::BarUnit) — that would be a type error paid
/// for forever, since a trade carries `price`/`size`/`side`, not OHLCV
///.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Trade {
    /// When the trade executed.
    pub ts: UnixNanos,
    /// The execution price, at the series' price scale.
    pub price: i64,
    /// The executed size, at the series' quantity scale.
    pub size: i64,
    /// The taker's side.
    pub side: Side,
}
