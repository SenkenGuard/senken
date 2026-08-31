//! Path construction under the data directory.
//!
//! Deliberately independent of `senken-marketdata`'s own `paths` module:
//! `senken-store` persists series for any `(source_id, symbol)` pair
//! without needing the instrument catalog, so it builds its own short
//! path-key-based helpers rather than pulling in a whole extra crate for
//! three string templates. `source_id` and `symbol` are both untrusted
//! input (a plugin author chooses them), so both go through
//! [`senken_core::path_key()`], exactly as `senken-marketdata`'s M2.3
//! helpers already do.
//!
//! No Arrow/Parquet dependency: pure string building.

use senken_core::path_key;
use senken_series::{Anchor, BarSpec, Origin, SeriesKey};

use crate::spec_token::encode_bars_dir_name;

/// The subtree owned entirely by one source: `sources/{source_id}`.
#[must_use]
pub fn source_dir(source_id: &str) -> String {
    format!("sources/{}", path_key(source_id))
}

/// The subtree for one instrument's own data within a source.
#[must_use]
pub fn instrument_dir(source_id: &str, symbol: &str) -> String {
    format!("{}/instruments/{}", source_dir(source_id), path_key(symbol))
}

/// The directory holding every coverage file for one bar series —
/// `sources/{source}/instruments/{symbol}/bars/{origin}-{spec}[@anchor]`.
///
/// `anchor` matters only for `Day` and coarser; it is
/// silently ignored below `Day`, matching [`Anchor`]'s own documented
/// scope, so callers never need to special-case sub-day specs.
#[must_use]
pub fn bars_dir(key: &SeriesKey, anchor: Anchor) -> String {
    format!(
        "{}/bars/{}",
        instrument_dir(&key.source_id, &key.symbol),
        encode_bars_dir_name(key.origin, key.spec, anchor)
    )
}

/// The directory holding every coverage file for one instrument's trades —
/// `sources/{source}/instruments/{symbol}/trades`.
///
/// Trades have no timeframe or origin to encode: there is
/// exactly one trade series per instrument.
#[must_use]
pub fn trades_dir(source_id: &str, symbol: &str) -> String {
    format!("{}/trades", instrument_dir(source_id, symbol))
}

/// The full path to one bar coverage file.
#[must_use]
pub fn bars_file(key: &SeriesKey, anchor: Anchor, range_token: &str) -> String {
    format!("{}/{range_token}.parquet", bars_dir(key, anchor))
}

/// The full path to one trades coverage file.
#[must_use]
pub fn trades_file(source_id: &str, symbol: &str, range_token: &str) -> String {
    format!("{}/{range_token}.parquet", trades_dir(source_id, symbol))
}

/// Rebuilds the [`BarSpec`] and [`Anchor`] a `bars/` subdirectory name
/// declares, given the series' source and symbol (which the directory
/// name itself does not carry — they come from its position in the tree).
///
/// Exists mainly for tests and diagnostics; [`crate::Store::coverage`]
/// does not need it, since a caller already names the exact series
/// (source, symbol, origin, spec, anchor) whose coverage it wants.
#[must_use]
pub fn parse_bars_dir_name(name: &str) -> Option<(Origin, BarSpec, Anchor)> {
    crate::spec_token::decode_bars_dir_name(name)
}

#[cfg(test)]
mod tests {
    use super::{bars_dir, bars_file, instrument_dir, source_dir, trades_dir};
    use senken_series::{Anchor, BarSpec, BarUnit, Origin, SeriesKey};

    #[test]
    fn paths_nest_under_the_source_like_marketdatas_do() {
        assert_eq!(source_dir("binance-spot"), "sources/binance-spot");
        assert_eq!(
            instrument_dir("binance-spot", "BTCUSDT"),
            "sources/binance-spot/instruments/BTCUSDT"
        );
    }

    #[test]
    fn source_id_and_symbol_are_both_path_encoded() {
        assert_eq!(source_dir("a/b"), "sources/a%2Fb");
        assert_eq!(
            instrument_dir("okx", "D.O.G.E."),
            "sources/okx/instruments/D%2EO%2EG%2EE%2E"
        );
    }

    #[test]
    fn bars_dir_includes_origin_and_spec() {
        let key = SeriesKey::new(
            "binance-spot",
            "BTCUSDT",
            Origin::Venue,
            BarSpec::new(1, BarUnit::Minute),
        );
        assert_eq!(
            bars_dir(&key, Anchor::UTC),
            "sources/binance-spot/instruments/BTCUSDT/bars/venue-1m"
        );
    }

    #[test]
    fn bars_dir_carries_a_non_utc_anchor_for_day_and_above() {
        let key = SeriesKey::new(
            "okx",
            "BTCUSDT",
            Origin::Venue,
            BarSpec::new(1, BarUnit::Day),
        );
        // OKX's UTC+8 (Hong Kong) day: `Anchor`'s own sign convention is
        // the negation of the venue's UTC offset — see `spec_token`'s
        // module docs for why.
        let anchor = Anchor::from_offset_nanos(-8 * 3_600_000_000_000);
        assert_eq!(
            bars_dir(&key, anchor),
            "sources/okx/instruments/BTCUSDT/bars/venue-1d@utc8"
        );
    }

    #[test]
    fn trades_dir_has_no_spec_or_origin() {
        assert_eq!(
            trades_dir("binance-spot", "BTCUSDT"),
            "sources/binance-spot/instruments/BTCUSDT/trades"
        );
    }

    #[test]
    fn bars_file_appends_the_range_token_and_extension() {
        let key = SeriesKey::new(
            "binance-spot",
            "BTCUSDT",
            Origin::Venue,
            BarSpec::new(1, BarUnit::Minute),
        );
        assert_eq!(
            bars_file(
                &key,
                Anchor::UTC,
                "20240101T000000000000000_20240201T000000000000000"
            ),
            "sources/binance-spot/instruments/BTCUSDT/bars/venue-1m/\
             20240101T000000000000000_20240201T000000000000000.parquet"
        );
    }
}
