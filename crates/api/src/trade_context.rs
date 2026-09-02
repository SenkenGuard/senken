//! What the trade engine is given on every call: the instrument catalog,
//! and a price to mark against.
//!
//! # Where a mark price comes from, and why
//!
//! From the newest stored bar's close on the finest series this
//! installation actually holds for the instrument, through the same
//! `senken-loader` ladder every chart reads. That choice is worth stating
//! because it has a visible consequence: **an instrument nobody has loaded
//! history for has no mark**, and a market order on it is refused by name
//! ([`TradeError::NoMarkPrice`](senken_trade::TradeError::NoMarkPrice))
//! rather than filled at a guess.
//!
//! The alternative was the live feed. It was not taken because this build
//! streams live prices for exactly one source — see `crate::feed`'s own
//! docs — so a live-only mark would leave paper trading working on OKX and
//! silently broken on the other twenty-one venues. Reading stored bars
//! works everywhere the platform has data, which is everywhere a user has
//! opened a chart.
//!
//! The staleness is not hidden: every [`MarkPrice`] carries the instant it
//! was current, so a caller can see that a mark is from Friday's close
//! rather than assume it is live.

use async_trait::async_trait;
use senken_core::decimal::Scaled;
use senken_core::{TimeRange, UnixNanos};
use senken_loader::{LoadError, SeriesLoader};
use senken_marketdata::{Instrument, InstrumentId};
use senken_series::{AggregateError, Anchor, BarSpec, BarUnit, Clock, Origin, SeriesKey};
use senken_trade::{InstrumentSource, MarkPrice, MarkPriceSource, TradeError};

use crate::AppState;

/// The windows a mark price lookup tries, in order, stopping at the first
/// that holds a bar.
///
/// Widening rather than one seven-day read, because `resolve` materialises
/// the whole range it is given: a week of one-minute bars is ten thousand
/// rows decoded on the request path of every order, to read the last one.
/// An instrument being traded normally answers from the first window.
///
/// The last window is a week so an instrument that stopped trading over a
/// weekend or a public holiday still marks at its last real price, rather
/// than dropping to "no price available" and refusing every order on
/// Monday morning.
const MARK_LOOKBACK_SECS: [i64; 4] = [60 * 60, 6 * 60 * 60, 24 * 60 * 60, 7 * 24 * 60 * 60];

/// The series a mark is read from, finest first.
///
/// **Not one-minute alone**, and the reason is a defect this cost: a reader
/// who opens an hourly chart has real, current prices stored for that
/// instrument, and asking only for a one-minute series found nothing — so
/// every market order came back "no price is available, load some history
/// first", which they had just done. Nothing on screen could have told them
/// the granularity was what mattered.
///
/// Finest first because a finer series carries a fresher close; a coarser
/// one is a legitimate, staler mark, and how stale is visible either way
/// because [`MarkPrice::as_of`] carries the bar's own instant rather than
/// the wall clock.
fn mark_specs() -> [BarSpec; 5] {
    [
        BarSpec::new(1, BarUnit::Minute),
        BarSpec::new(5, BarUnit::Minute),
        BarSpec::new(15, BarUnit::Minute),
        BarSpec::new(1, BarUnit::Hour),
        BarSpec::new(1, BarUnit::Day),
    ]
}

/// The engine's window onto the instrument catalog.
pub(crate) struct CatalogInstruments {
    state: AppState,
}

impl CatalogInstruments {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }
}

#[async_trait]
impl InstrumentSource for CatalogInstruments {
    async fn instrument(&self, id: &InstrumentId) -> Result<Option<Instrument>, TradeError> {
        match self.state.runtime.marketdata().instrument(id).await {
            Ok(hit) => Ok(hit.map(|hit| hit.instrument)),
            // An unknown source is an absence, not a failure: the caller
            // turns it into "this instrument is not tradable", which is
            // what it means.
            Err(senken_marketdata::MarketDataError::UnknownSource(_)) => Ok(None),
            Err(source) => Err(TradeError::adapter(source.to_string())),
        }
    }
}

/// The engine's window onto stored prices.
pub(crate) struct StoredMarkPrice {
    state: AppState,
}

impl StoredMarkPrice {
    pub(crate) fn new(state: AppState) -> Self {
        Self { state }
    }

    /// The loader for `id`'s source, or `None` when no plugin registered a
    /// bar source for it.
    fn loader(&self, id: &InstrumentId) -> Option<SeriesLoader> {
        self.state.runtime.series().loader(id.source()).cloned()
    }
}

#[async_trait]
impl MarkPriceSource for StoredMarkPrice {
    async fn mark_price(&self, instrument: &InstrumentId) -> Result<Option<MarkPrice>, TradeError> {
        let Some(loader) = self.loader(instrument) else {
            return Ok(None);
        };
        let Ok(Some(hit)) = self.state.runtime.marketdata().instrument(instrument).await else {
            return Ok(None);
        };

        let now = senken_loader::SystemClock.now();

        // Window-major: a narrow window across every granularity before a
        // wider one at any of them, so the answer is the freshest price
        // this installation actually holds rather than the finest series it
        // happens to have somewhere in the distant past.
        for lookback in MARK_LOOKBACK_SECS {
            let Some(from) = UnixNanos::from_secs(now.as_nanos() / 1_000_000_000 - lookback) else {
                return Ok(None);
            };
            let Some(range) = TimeRange::new(from, now) else {
                return Ok(None);
            };

            for spec in mark_specs() {
                let key = SeriesKey::new(
                    instrument.source(),
                    instrument.symbol(),
                    Origin::Derived,
                    spec,
                );
                // `resolve` never fetches — it returns whatever is already
                // stored. A mark price must not be able to start a
                // multi-minute backfill on the request path of an order.
                let resolved = match loader.resolve(&key, range, Anchor::UTC).await {
                    Ok(resolved) => resolved,
                    // A venue whose finest candle is coarser than this spec
                    // cannot fold one out of what it has, and says so by
                    // name. That is an absence, not a failure: the next
                    // spec in the ladder is exactly the coarser one this
                    // venue *can* answer, and turning it into an error
                    // instead made every market order on such a venue fail
                    // with "internal server error".
                    Err(LoadError::Aggregate(AggregateError::DoesNotDivide { .. })) => continue,
                    Err(source) => return Err(TradeError::adapter(source.to_string())),
                };

                if let Some(bar) = resolved.bars.last() {
                    return Ok(Some(MarkPrice {
                        price: Scaled::new(hit.instrument.price_scale, bar.close),
                        // The bar's own open time, not the wall clock: this
                        // is when the price was current, and reporting it as
                        // "now" would make a stale mark indistinguishable
                        // from a live one.
                        as_of: bar.ts_open,
                    }));
                }
            }
        }
        Ok(None)
    }
}
