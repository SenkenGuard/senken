//! One source's instruments, indexed for lookup.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::Arc;

use chrono::{DateTime, Utc};

use crate::instrument::Instrument;

/// One source's instrument list, held in memory and indexed for lookup.
///
/// Built once per load. Search scans [`instruments`](Self::instruments) in
/// order; [`find`](Self::find) answers exact lookups without a scan. The
/// index stores positions rather than clones, so each instrument exists
/// exactly once in memory.
///
/// Symbols are unique within a catalog: [`new`](Self::new) enforces the
/// uniqueness [`Instrument::symbol`] promises, so every instrument here is
/// addressable by its symbol and its id round-trips through search.
#[derive(Debug)]
pub struct SourceCatalog {
    source_id: Arc<str>,
    source_name: Arc<str>,
    synced_at: DateTime<Utc>,
    instruments: Vec<Instrument>,
    by_symbol: HashMap<Box<str>, usize>,
}

impl SourceCatalog {
    /// Indexes `instruments`, keeping venue order. When two share a symbol
    /// (case-insensitively) only the first is kept; the duplicate is dropped
    /// and logged at `warn`, since a symbol that names two instruments can
    /// satisfy neither lookup nor a stable [`InstrumentId`].
    ///
    /// [`InstrumentId`]: crate::id::InstrumentId
    #[must_use]
    pub fn new(
        source_id: impl Into<Arc<str>>,
        source_name: impl Into<Arc<str>>,
        synced_at: DateTime<Utc>,
        instruments: Vec<Instrument>,
    ) -> Self {
        let source_id = source_id.into();
        let source_name = source_name.into();

        let mut kept: Vec<Instrument> = Vec::with_capacity(instruments.len());
        let mut by_symbol = HashMap::with_capacity(instruments.len());
        for instrument in instruments {
            match by_symbol.entry(instrument.symbol.to_ascii_uppercase().into_boxed_str()) {
                Entry::Vacant(slot) => {
                    slot.insert(kept.len());
                    kept.push(instrument);
                }
                Entry::Occupied(slot) => {
                    let first: &Instrument = &kept[*slot.get()];
                    tracing::warn!(
                        source = %source_id,
                        symbol = instrument.symbol,
                        kept = first.source_symbol,
                        dropped = instrument.source_symbol,
                        "duplicate symbol in catalog; dropping all but the first"
                    );
                }
            }
        }

        Self {
            source_id,
            source_name,
            synced_at,
            instruments: kept,
            by_symbol,
        }
    }

    /// The owning source's id.
    #[must_use]
    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    /// The owning source's display name.
    #[must_use]
    pub fn source_name(&self) -> &str {
        &self.source_name
    }

    /// The display name behind the shared handle search results hold.
    pub(crate) fn source_name_shared(&self) -> Arc<str> {
        Arc::clone(&self.source_name)
    }

    /// When this catalog was fetched from the venue.
    #[must_use]
    pub fn synced_at(&self) -> DateTime<Utc> {
        self.synced_at
    }

    /// Number of instruments, whatever their status.
    #[must_use]
    pub fn len(&self) -> usize {
        self.instruments.len()
    }

    /// `true` when the catalog holds no instruments.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.instruments.is_empty()
    }

    /// All instruments in venue order.
    #[must_use]
    pub fn instruments(&self) -> &[Instrument] {
        &self.instruments
    }

    /// Exact symbol lookup, case-insensitive. O(1) and allocation-free for
    /// symbols up to 64 bytes.
    #[must_use]
    pub fn find(&self, symbol: &str) -> Option<&Instrument> {
        let position = with_ascii_uppercase(symbol, |upper| self.by_symbol.get(upper).copied())?;
        self.instruments.get(position)
    }
}

/// Calls `f` with `s` upper-cased, using a stack buffer when it fits.
fn with_ascii_uppercase<R>(s: &str, f: impl FnOnce(&str) -> R) -> R {
    const STACK: usize = 64;
    if s.len() <= STACK {
        let mut buf = [0_u8; STACK];
        let buf = &mut buf[..s.len()];
        buf.copy_from_slice(s.as_bytes());
        buf.make_ascii_uppercase();
        // ASCII upper-casing never breaks UTF-8; the check is for the compiler.
        f(std::str::from_utf8(buf).unwrap_or(s))
    } else {
        f(&s.to_ascii_uppercase())
    }
}

#[cfg(test)]
mod tests {
    use super::{SourceCatalog, with_ascii_uppercase};
    use crate::instrument::{Instrument, InstrumentStatus};
    use chrono::Utc;

    fn instrument(symbol: &str) -> Instrument {
        let base = symbol.split('-').next().unwrap_or(symbol);
        Instrument::spot(symbol.replace('-', ""), symbol, base, "USDT")
            .with_name(format!("{symbol} spot"))
            .with_status(InstrumentStatus::Trading)
            .with_price_increment((2, 1))
            .with_qty_increment((2, 1))
    }

    fn catalog() -> SourceCatalog {
        SourceCatalog::new(
            "okx",
            "OKX",
            Utc::now(),
            vec![instrument("BTC-USDT"), instrument("ETH-USDT")],
        )
    }

    #[test]
    fn exact_lookup_ignores_case() {
        let catalog = catalog();
        assert!(catalog.find("BTCUSDT").is_some());
        assert!(catalog.find("btcusdt").is_some());
        assert!(catalog.find("BtCUsDt").is_some());
        assert!(catalog.find("SOLUSDT").is_none());
    }

    #[test]
    fn lookup_works_past_the_stack_buffer() {
        let long = "x".repeat(100);
        let catalog = SourceCatalog::new("okx", "OKX", Utc::now(), vec![instrument(&long)]);
        assert!(catalog.find(&long.to_uppercase()).is_some());
        assert!(catalog.find(&long).is_some());
    }

    #[test]
    fn upper_casing_preserves_non_ascii() {
        with_ascii_uppercase("btc-€", |u| assert_eq!(u, "BTC-€"));
    }

    #[test]
    fn duplicate_symbols_are_dropped_keeping_the_first() {
        let catalog = SourceCatalog::new(
            "okx",
            "OKX",
            Utc::now(),
            vec![instrument("BTC-USDT"), instrument("BTCUSDT")],
        );

        assert_eq!(catalog.len(), 1, "the duplicate must not survive");
        assert_eq!(
            catalog.find("BTCUSDT").unwrap().source_symbol,
            "BTC-USDT",
            "the first one keeps the symbol"
        );
        assert_eq!(
            catalog.instruments().len(),
            1,
            "search must never see an instrument whose id cannot round-trip"
        );
    }

    #[test]
    fn the_index_points_at_the_right_instrument() {
        let catalog = catalog();
        assert_eq!(catalog.find("ethusdt").unwrap().symbol, "ETHUSDT");
        assert_eq!(catalog.len(), 2);
    }
}
