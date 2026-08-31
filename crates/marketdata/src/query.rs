//! Search parameters and ranking.

use crate::instrument::{Instrument, InstrumentKind};

/// What to search for, and which slice of the results to return.
///
/// Build one from free text with [`new`](Self::new) (or `From<&str>`), or
/// start from [`all`](Self::all) and narrow with the builder methods.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstrumentQuery {
    source: Option<String>,
    text: Option<String>,
    /// Kinds to accept. Empty means every kind.
    kinds: Vec<InstrumentKind>,
    offset: usize,
    limit: Option<usize>,
}

impl InstrumentQuery {
    /// Matches every searchable instrument from every source.
    #[must_use]
    pub fn all() -> Self {
        Self::default()
    }

    /// Builds a query from one search box worth of input.
    ///
    /// A `source:term` prefix narrows the search to sources whose id starts
    /// with `source` (case-insensitively); the remainder is matched against
    /// symbol, venue symbol, base, quote, source id and source name. Empty
    /// input lists everything.
    ///
    /// # Examples
    /// ```
    /// use senken_marketdata::InstrumentQuery;
    ///
    /// let query = InstrumentQuery::new("okx:btc");
    /// assert_eq!(query.source(), Some("okx"));
    /// assert_eq!(query.term(), Some("btc"));
    /// ```
    #[must_use]
    pub fn new(raw: impl AsRef<str>) -> Self {
        let raw = raw.as_ref().trim();
        if raw.is_empty() {
            return Self::all();
        }

        let (source, term) = match raw.split_once(':') {
            Some((source, term)) => (source.trim(), term.trim()),
            None => ("", raw),
        };

        Self {
            source: (!source.is_empty()).then(|| source.to_string()),
            text: (!term.is_empty()).then(|| term.to_string()),
            ..Self::default()
        }
    }

    /// Restricts results to sources whose id starts with `source`.
    #[must_use]
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Sets the free-text term.
    #[must_use]
    pub fn with_term(mut self, term: impl Into<String>) -> Self {
        self.text = Some(term.into());
        self
    }

    /// Restricts results to one kind of instrument. Call it more than once
    /// to accept several kinds; with no call at all, every kind matches.
    #[must_use]
    pub fn with_kind(mut self, kind: InstrumentKind) -> Self {
        if !self.kinds.contains(&kind) {
            self.kinds.push(kind);
        }
        self
    }

    /// The kinds this query accepts. Empty means every kind.
    #[must_use]
    pub fn kinds(&self) -> &[InstrumentKind] {
        &self.kinds
    }

    /// Returns at most `limit` matches.
    #[must_use]
    pub fn with_limit(mut self, limit: usize) -> Self {
        self.limit = Some(limit);
        self
    }

    /// Skips the first `offset` matches.
    #[must_use]
    pub fn with_offset(mut self, offset: usize) -> Self {
        self.offset = offset;
        self
    }

    /// Sets offset and limit from a zero-based page number and page size.
    #[must_use]
    pub fn with_page(self, page: usize, per_page: usize) -> Self {
        self.with_offset(page.saturating_mul(per_page))
            .with_limit(per_page)
    }

    /// The source prefix, if any.
    #[must_use]
    pub fn source(&self) -> Option<&str> {
        self.source.as_deref()
    }

    /// The free-text term, if any.
    #[must_use]
    pub fn term(&self) -> Option<&str> {
        self.text.as_deref()
    }

    /// Matches to skip.
    #[must_use]
    pub fn offset(&self) -> usize {
        self.offset
    }

    /// Maximum matches to return, if bounded.
    #[must_use]
    pub fn limit(&self) -> Option<usize> {
        self.limit
    }

    /// `true` when `source_id` passes the source prefix, if one is set.
    #[must_use]
    pub fn accepts_source(&self, source_id: &str) -> bool {
        match &self.source {
            None => true,
            Some(hint) => starts_with_ignore_case(source_id, hint),
        }
    }

    /// `true` when the term matches the source itself rather than any one
    /// instrument — its id or its display name.
    ///
    /// The answer is the same for every instrument in a catalog, so
    /// [`rank`](Self::rank) takes it as an argument instead of recomputing
    /// it per row: at forty thousand instruments those two string scans are
    /// the difference between two comparisons and eighty thousand.
    #[must_use]
    pub fn matches_source(&self, source_id: &str, source_name: &str) -> bool {
        match &self.text {
            None => true,
            Some(term) => {
                contains_ignore_case(source_id, term) || contains_ignore_case(source_name, term)
            }
        }
    }

    /// Ranks `instrument` against this query, or `None` when it must not
    /// appear in the results: its status is unsearchable, its kind is
    /// filtered out, or nothing about it matches the term.
    ///
    /// One pass over the fields, strongest signal first. `source_matches`
    /// comes from [`matches_source`](Self::matches_source), hoisted out of
    /// the caller's loop. Source *selection* is a separate, earlier filter
    ///   — see [`accepts_source`](Self::accepts_source).
    #[must_use]
    pub fn rank(&self, instrument: &Instrument, source_matches: bool) -> Option<MatchRank> {
        if !instrument.status.is_searchable() {
            return None;
        }
        if !self.kinds.is_empty() && !self.kinds.contains(&instrument.kind) {
            return None;
        }
        let Some(term) = &self.text else {
            return Some(MatchRank::Listed);
        };

        // Screen before classifying: on a selective term almost every
        // instrument fails every `contains`, and rejecting is the hot path.
        if !(source_matches
            || contains_ignore_case(&instrument.symbol, term)
            || contains_ignore_case(&instrument.source_symbol, term)
            || contains_ignore_case(&instrument.base, term)
            || contains_ignore_case(&instrument.quote, term))
        {
            return None;
        }

        if instrument.symbol.eq_ignore_ascii_case(term)
            || instrument.source_symbol.eq_ignore_ascii_case(term)
        {
            return Some(MatchRank::ExactSymbol);
        }
        if instrument.base.eq_ignore_ascii_case(term) {
            return Some(MatchRank::ExactBase);
        }
        if starts_with_ignore_case(&instrument.symbol, term)
            || starts_with_ignore_case(&instrument.source_symbol, term)
        {
            return Some(MatchRank::SymbolPrefix);
        }
        Some(MatchRank::Contains)
    }
}

impl From<&str> for InstrumentQuery {
    fn from(raw: &str) -> Self {
        Self::new(raw)
    }
}

impl From<String> for InstrumentQuery {
    fn from(raw: String) -> Self {
        Self::new(raw)
    }
}

/// How closely an instrument matches a query term.
///
/// **The declaration order is load-bearing**: the derived [`Ord`] is what
/// sorts search results, best match first. Add new variants where they
/// belong in that order, never at the end by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[non_exhaustive]
pub enum MatchRank {
    /// The term equals the symbol or venue symbol.
    ExactSymbol,
    /// The term equals the base asset.
    ExactBase,
    /// The symbol or venue symbol starts with the term.
    SymbolPrefix,
    /// The term appears somewhere in a searchable field.
    Contains,
    /// No term was given; every instrument is an equal match.
    Listed,
}

pub(crate) fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    if needle.is_empty() {
        return true;
    }
    let haystack = haystack.as_bytes();
    if needle.len() > haystack.len() {
        return false;
    }
    haystack
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle))
}

pub(crate) fn starts_with_ignore_case(haystack: &str, needle: &str) -> bool {
    let needle = needle.as_bytes();
    let haystack = haystack.as_bytes();
    haystack.len() >= needle.len() && haystack[..needle.len()].eq_ignore_ascii_case(needle)
}

#[cfg(test)]
mod tests {
    use super::{InstrumentQuery, MatchRank, contains_ignore_case, starts_with_ignore_case};
    use crate::instrument::{Instrument, InstrumentStatus};

    fn instrument(symbol: &str, base: &str, quote: &str) -> Instrument {
        Instrument::spot(symbol, format!("{base}-{quote}"), base, quote)
            .with_status(InstrumentStatus::Trading)
            .with_price_increment((2, 1))
            .with_qty_increment((2, 1))
    }

    #[test]
    fn case_insensitive_helpers_do_not_allocate_or_panic() {
        assert!(contains_ignore_case("BTCUSDT", "btc"));
        assert!(contains_ignore_case("BTCUSDT", "USD"));
        assert!(!contains_ignore_case("BTC", "BTCUSDT"));
        assert!(contains_ignore_case("anything", ""));
        assert!(starts_with_ignore_case("BTCUSDT", "btc"));
        assert!(!starts_with_ignore_case("ETHUSDT", "btc"));
    }

    #[test]
    fn text_matches_symbol_base_quote_and_source() {
        let btc = instrument("BTCUSDT", "BTC", "USDT");

        let ranks = |raw: &str| {
            let query = InstrumentQuery::new(raw);
            query.rank(&btc, query.matches_source("binance-spot", "Binance Spot"))
        };
        assert!(ranks("btc").is_some());
        assert!(ranks("usdt").is_some());
        assert!(ranks("binance").is_some());
        assert!(ranks("Binance Spot").is_some());
        assert!(ranks("solana").is_none());
    }

    #[test]
    fn the_raw_venue_symbol_is_searchable_and_ranks_as_exact() {
        // `symbol` is normalised ("BTCUSDT"), `source_symbol` is verbatim
        // ("BTC-USDT"); a user may paste either one.
        let btc = instrument("BTCUSDT", "BTC", "USDT");
        let paste_symbol = InstrumentQuery::new("BTCUSDT");
        assert!(
            paste_symbol
                .rank(&btc, paste_symbol.matches_source("okx", "OKX"))
                .is_some()
        );
        assert_eq!(
            {
                let q = InstrumentQuery::new("btc-usdt");
                q.rank(&btc, q.matches_source("okx", "OKX"))
            },
            Some(MatchRank::ExactSymbol)
        );
    }

    #[test]
    fn unsearchable_statuses_are_hidden() {
        let mut test_pair = instrument("TESTUSDT", "TEST", "USDT");
        test_pair.status = InstrumentStatus::Test;
        let query = InstrumentQuery::new("test");
        assert!(
            query
                .rank(&test_pair, query.matches_source("okx", "OKX"))
                .is_none()
        );
    }

    #[test]
    fn halted_instruments_stay_searchable() {
        let mut halted = instrument("BTCUSDT", "BTC", "USDT");
        halted.status = InstrumentStatus::Halted;
        let query = InstrumentQuery::new("btc");
        assert!(
            query
                .rank(&halted, query.matches_source("okx", "OKX"))
                .is_some()
        );
    }

    #[test]
    fn ranking_prefers_exact_then_prefix_then_substring() {
        let query = InstrumentQuery::new("btc");

        assert_eq!(
            query.rank(
                &instrument("BTC", "BTC", "USDT"),
                query.matches_source("x", "X")
            ),
            Some(MatchRank::ExactSymbol)
        );
        assert_eq!(
            query.rank(
                &instrument("BTCUSDT", "BTC", "USDT"),
                query.matches_source("x", "X")
            ),
            Some(MatchRank::ExactBase)
        );
        assert_eq!(
            query.rank(
                &instrument("BTCB", "BTCB", "USDT"),
                query.matches_source("x", "X")
            ),
            Some(MatchRank::SymbolPrefix)
        );
        assert_eq!(
            query.rank(
                &instrument("WBTCUSDT", "WBTC", "USDT"),
                query.matches_source("x", "X")
            ),
            Some(MatchRank::Contains)
        );
        assert_eq!(
            query.rank(
                &instrument("ETHUSDT", "ETH", "USDT"),
                query.matches_source("x", "X")
            ),
            None
        );
    }

    #[test]
    fn rank_order_is_best_first() {
        assert!(MatchRank::ExactSymbol < MatchRank::ExactBase);
        assert!(MatchRank::ExactBase < MatchRank::SymbolPrefix);
        assert!(MatchRank::SymbolPrefix < MatchRank::Contains);
        assert!(MatchRank::Contains < MatchRank::Listed);
    }

    #[test]
    fn a_colon_narrows_the_search_to_one_source() {
        let q = InstrumentQuery::new("okx:btc");
        assert_eq!(q.source(), Some("okx"));
        assert_eq!(q.term(), Some("btc"));
        assert!(q.accepts_source("okx"));
        assert!(!q.accepts_source("binance-spot"));
    }

    #[test]
    fn source_hint_matches_by_prefix() {
        let q = InstrumentQuery::new("BINANCE:xaut");
        assert!(q.accepts_source("binance-spot"));
        assert!(q.accepts_source("binance-futures"));
        assert!(!q.accepts_source("okx"));
    }

    #[test]
    fn builder_and_text_forms_agree() {
        assert_eq!(
            InstrumentQuery::new("okx:btc"),
            InstrumentQuery::all().with_source("okx").with_term("btc")
        );
    }

    #[test]
    fn a_bare_hint_lists_everything_from_that_source() {
        let q = InstrumentQuery::new("okx:");
        assert_eq!(q.source(), Some("okx"));
        assert!(q.term().is_none());
    }

    #[test]
    fn a_leading_colon_is_just_a_term() {
        let q = InstrumentQuery::new(":btc");
        assert!(q.source().is_none());
        assert_eq!(q.term(), Some("btc"));
    }

    #[test]
    fn only_the_first_colon_splits() {
        let q = InstrumentQuery::new("okx:btc:usdt");
        assert_eq!(q.source(), Some("okx"));
        assert_eq!(q.term(), Some("btc:usdt"));
    }

    #[test]
    fn empty_text_becomes_a_list_everything_query() {
        let query = InstrumentQuery::from("   ");
        assert!(query.term().is_none());
        assert_eq!(
            query.rank(
                &instrument("BTCUSDT", "BTC", "USDT"),
                query.matches_source("okx", "OKX")
            ),
            Some(MatchRank::Listed)
        );
    }

    #[test]
    fn a_kind_filter_narrows_to_that_kind() {
        use crate::instrument::{Contract, InstrumentKind, Settlement};

        let spot = instrument("BTCUSDT", "BTC", "USDT");
        let perp = Instrument::derivative(
            "BTCUSDT",
            "BTC-USDT-SWAP",
            "BTC",
            "USDT",
            InstrumentKind::Perpetual,
            Contract::new("USDT", Settlement::Linear),
        )
        .with_status(InstrumentStatus::Trading);

        let any = InstrumentQuery::new("btc");
        assert!(any.rank(&spot, any.matches_source("x", "X")).is_some());
        assert!(any.rank(&perp, any.matches_source("x", "X")).is_some());

        let perps = InstrumentQuery::new("btc").with_kind(InstrumentKind::Perpetual);
        assert!(perps.rank(&spot, perps.matches_source("x", "X")).is_none());
        assert!(perps.rank(&perp, perps.matches_source("x", "X")).is_some());

        let both = perps.clone().with_kind(InstrumentKind::Spot);
        assert!(both.rank(&spot, both.matches_source("x", "X")).is_some());
        assert!(both.rank(&perp, both.matches_source("x", "X")).is_some());
        assert_eq!(both.kinds().len(), 2);

        let repeated = perps.with_kind(InstrumentKind::Perpetual);
        assert_eq!(repeated.kinds().len(), 1, "a kind is never listed twice");
    }

    #[test]
    fn page_translates_to_offset_and_limit() {
        let first = InstrumentQuery::new("btc").with_page(0, 20);
        assert_eq!(first.offset(), 0);
        assert_eq!(first.limit(), Some(20));

        let third = InstrumentQuery::new("btc").with_page(2, 20);
        assert_eq!(third.offset(), 40);
        assert_eq!(third.limit(), Some(20));
    }
}
