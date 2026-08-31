//! Filesystem paths under the on-disk data directory
//!.
//!
//! One function per path shape, so nothing downstream builds a path by
//! concatenating strings by hand. `source_id` is untrusted input exactly
//! like a symbol is — a plugin author chooses it, and it flows straight
//! into a directory name — so it goes through [`path_key()`] here too, not
//! only the symbol half.

use senken_core::path_key;

/// The subtree owned entirely by one source: `sources/{source_id}`.
#[must_use]
pub fn source_dir(source_id: &str) -> String {
    format!("sources/{}", path_key(source_id))
}

/// Where a source's instrument catalog snapshot lives.
#[must_use]
pub fn instruments_path(source_id: &str) -> String {
    format!("{}/instruments.json", source_dir(source_id))
}

/// The subtree for one instrument's own data (bars, trades, …) within a
/// source.
#[must_use]
pub fn instrument_dir(source_id: &str, symbol: &str) -> String {
    format!("{}/instruments/{}", source_dir(source_id), path_key(symbol))
}

#[cfg(test)]
mod tests {
    use super::{instrument_dir, instruments_path, source_dir};

    #[test]
    fn paths_nest_under_the_source() {
        assert_eq!(source_dir("binance-spot"), "sources/binance-spot");
        assert_eq!(
            instruments_path("binance-spot"),
            "sources/binance-spot/instruments.json"
        );
        assert_eq!(
            instrument_dir("binance-spot", "BTCUSDT"),
            "sources/binance-spot/instruments/BTCUSDT"
        );
    }

    #[test]
    fn source_id_is_path_encoded_like_a_symbol() {
        // `source_id` is untrusted input too: a hostile or buggy
        // plugin could register one containing a path separator or a
        // Windows device name, and it must not escape `sources/` or
        // collide with a reserved name.
        assert_eq!(source_dir("a/b"), "sources/a%2Fb");
        assert_eq!(source_dir("CON"), "sources/%43ON");
    }

    #[test]
    fn the_symbol_half_of_an_instrument_dir_is_also_path_encoded() {
        assert_eq!(
            instrument_dir("okx", "D.O.G.E."),
            "sources/okx/instruments/D%2EO%2EG%2EE%2E"
        );
    }
}
