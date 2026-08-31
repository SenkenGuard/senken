//! The registry half of the crate: [`MarketData`], cached catalogs and
//! cross-source search. Compiled only with the `registry` feature.

use std::collections::hash_map::Entry;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::stream::{self, StreamExt};
use senken_storage::{Snapshot, Storage, StorageError};
use tokio::sync::OnceCell;

use crate::catalog::SourceCatalog;
use crate::id::{InstrumentId, InstrumentIdError};
use crate::instrument::Instrument;
use crate::paths::instruments_path;
use crate::query::{InstrumentQuery, MatchRank};
use crate::source::{MarketDataSource, SourceDetail, SourceError, SourceSummary};

/// Layout version of the on-disk instrument snapshot. Bump when
/// [`Instrument`] changes incompatibly — including a change in what a field
/// *means* — and older snapshots are refetched.
///
/// History: 6 — the layout moved from `.data/marketdata/sources/…` to
/// `.data/sources/…`, and [`Contract::expiry`](crate::Contract::expiry)
/// became a checked [`UnixNanos`](senken_core::UnixNanos) instead of raw
/// milliseconds — the exact seconds-vs-milliseconds confusion `UnixNanos`
/// exists to make unrepresentable was live in Gate's plugin until this
/// version.
/// 5 — normalised symbols keep punctuation that belongs to a
/// token's name, so `$U-USDT` and `U-USDT` no longer collapse onto one
/// symbol; cached catalogs hold the old, collided form.
/// 4 — instruments gained [`Contract`](crate::Contract), so a
/// source may now report derivatives; an older build would silently drop
/// those terms and read an inverse perpetual as if it were spot.
/// 3 — `price_scale`/`qty_scale` became the minimal scale derived from the
/// tick/step (see [`Instrument`]); earlier snapshots from some venues
/// carried the venue's asset precision instead.
pub const INSTRUMENTS_SCHEMA_VERSION: u32 = 6;

/// How long a cached catalog is trusted before the venue is asked again.
pub const DEFAULT_CACHE_TTL: Duration = Duration::from_hours(24);

/// How many sources are loaded at the same time by one search.
const SOURCE_CONCURRENCY: usize = 8;

/// Everything [`MarketData`] can fail with.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MarketDataError {
    /// A source failed to deliver its instruments.
    #[error(transparent)]
    Source(#[from] SourceError),

    /// The on-disk cache could not be read or written.
    #[error(transparent)]
    Storage(#[from] StorageError),

    /// An instrument id could not be parsed.
    #[error(transparent)]
    Id(#[from] InstrumentIdError),

    /// No registered source has this id.
    #[error("unknown source `{0}`")]
    UnknownSource(String),

    /// A source with this id is already registered.
    #[error("source `{0}` is already registered")]
    DuplicateSource(String),

    /// A source reported an id that cannot form an [`InstrumentId`].
    #[error("source id `{0}` must be lowercase [a-z0-9-]")]
    InvalidSourceId(String),

    /// Two instruments from one source normalised to the same symbol.
    ///
    /// This is a plugin bug, not a runtime condition to route around:
    /// `(source_id, symbol)` is the key every persisted path is built from
    /// , so two instruments sharing one would silently
    /// interleave their bars on disk. The fix is to split the source, as
    /// Phemex was split into `phemex-spot` and `phemex-perp` when
    /// `sOLUSDT` collided with `SOLUSDT` — never to keep one arbitrarily.
    #[error(
        "source `{source_id}` reports symbol `{symbol}` twice: `{first_source_symbol}` and `{second_source_symbol}`"
    )]
    DuplicateSymbol {
        /// The source that reported both instruments.
        source_id: String,
        /// The normalised symbol both instruments collide on.
        symbol: String,
        /// The venue's own identifier for the instrument seen first.
        first_source_symbol: String,
        /// The venue's own identifier for the instrument seen second.
        second_source_symbol: String,
    },
}

/// One source that could not contribute to a search.
#[derive(Debug)]
pub struct SourceFailure {
    /// The failing source.
    pub source_id: String,
    /// Why it failed.
    pub error: MarketDataError,
}

/// One search hit.
#[derive(Debug, Clone)]
pub struct InstrumentMatch {
    /// Fully-qualified id; `id.source()` is the source id.
    pub id: InstrumentId,
    /// Display name of the source.
    pub source_name: Arc<str>,
    /// The instrument itself.
    pub instrument: Instrument,
}

impl InstrumentMatch {
    /// The id of the source this hit came from.
    #[must_use]
    pub fn source_id(&self) -> &str {
        self.id.source()
    }
}

/// One page of search results plus everything that went wrong producing it.
#[derive(Debug, Default)]
pub struct InstrumentPage {
    /// The hits on this page, best match first.
    pub matches: Vec<InstrumentMatch>,
    /// Hits across all pages.
    pub total_matched: usize,
    /// Position of the first hit on this page within `total_matched`.
    pub offset: usize,
    /// Sources that failed to load; their instruments are absent.
    pub failures: Vec<SourceFailure>,
}

impl InstrumentPage {
    /// `true` when every selected source contributed.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        self.failures.is_empty()
    }

    /// `true` when a later page exists.
    #[must_use]
    pub fn has_more(&self) -> bool {
        self.offset + self.matches.len() < self.total_matched
    }
}

type CatalogCell = Arc<OnceCell<Arc<SourceCatalog>>>;

/// A registry of sources with a cached catalog per source.
///
/// Cheap to share behind an `Arc`; every method except
/// [`register_source`](Self::register_source) takes `&self`.
///
/// # Examples
///
/// Implement [`MarketDataSource`] for a venue, register it, and search:
///
/// ```
/// use std::sync::Arc;
///
/// use senken_marketdata::{
///     Instrument, InstrumentQuery, InstrumentStatus, MarketData, MarketDataSource,
///     SourceError,
/// };
/// use senken_storage::Storage;
///
/// struct Demo;
///
/// #[async_trait::async_trait]
/// impl MarketDataSource for Demo {
///     fn id(&self) -> &str {
///         "demo"
///     }
///
///     fn name(&self) -> &str {
///         "Demo Venue"
///     }
///
///     async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
///         Ok(vec![
///             Instrument::spot("BTCUSDT", "BTC-USDT", "BTC", "USDT")
///                 .with_status(InstrumentStatus::Trading)
///                 .with_price_increment((1, 1))
///                 .with_qty_increment((8, 1)),
///         ])
///     }
/// }
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let dir = tempfile::tempdir()?;
/// let storage = Storage::new(dir.path());
/// storage.init()?;
///
/// let mut marketdata = MarketData::new(Arc::new(storage));
/// marketdata.register_source(Arc::new(Demo))?;
///
/// let page = marketdata.instruments(InstrumentQuery::new("btc")).await;
/// assert_eq!(page.matches[0].id.as_str(), "demo:BTCUSDT");
/// # Ok(())
/// # }
/// ```
pub struct MarketData {
    sources: Vec<Arc<dyn MarketDataSource>>,
    storage: Arc<Storage>,
    cache_ttl: Duration,
    /// One cell per source id. The cell is the single-flight guard: the
    /// first caller runs the load, everyone else awaits it, and a failed
    /// load leaves the cell empty so the next call retries.
    catalogs: Mutex<HashMap<String, CatalogCell>>,
}

impl MarketData {
    /// Creates an empty registry caching into `storage` with
    /// [`DEFAULT_CACHE_TTL`].
    #[must_use]
    pub fn new(storage: Arc<Storage>) -> Self {
        Self {
            sources: Vec::new(),
            storage,
            cache_ttl: DEFAULT_CACHE_TTL,
            catalogs: Mutex::new(HashMap::new()),
        }
    }

    /// Changes how long an on-disk catalog is trusted before refetching.
    #[must_use]
    pub fn with_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = ttl;
        self
    }

    /// Adds a source. Its id must be lowercase `[a-z0-9-]` and unique.
    ///
    /// # Errors
    /// [`MarketDataError::InvalidSourceId`] or
    /// [`MarketDataError::DuplicateSource`].
    pub fn register_source(
        &mut self,
        source: Arc<dyn MarketDataSource>,
    ) -> Result<(), MarketDataError> {
        let id = source.id();
        if !InstrumentId::is_valid_source(id) {
            return Err(MarketDataError::InvalidSourceId(id.to_owned()));
        }
        if self.find_source(id).is_some() {
            return Err(MarketDataError::DuplicateSource(id.to_owned()));
        }
        self.sources.push(source);
        Ok(())
    }

    /// Registered sources. Cheap: no disk, no network.
    #[must_use]
    pub fn sources(&self) -> Vec<SourceSummary> {
        self.sources
            .iter()
            .map(|source| SourceSummary {
                id: source.id().to_owned(),
                name: source.name().to_owned(),
            })
            .collect()
    }

    /// One source with its catalog statistics. Loads the catalog if needed.
    ///
    /// # Errors
    /// [`MarketDataError::UnknownSource`], or whatever loading failed with.
    pub async fn source_detail(&self, source_id: &str) -> Result<SourceDetail, MarketDataError> {
        let source = self
            .find_source(source_id)
            .ok_or_else(|| MarketDataError::UnknownSource(source_id.to_owned()))?;
        let catalog = self.catalog_of(source.as_ref()).await?;
        Ok(describe(&catalog))
    }

    /// One instrument by its fully-qualified id. O(1) once the catalog is
    /// warm. Accepts an [`InstrumentId`], a reference to one, or anything
    /// that parses into one such as `"okx:BTCUSDT"`.
    ///
    /// # Errors
    /// [`MarketDataError::Id`] if `id` does not parse,
    /// [`MarketDataError::UnknownSource`] if no source has that id, or
    /// whatever loading the catalog failed with. An unknown symbol on a known
    /// source is `Ok(None)`.
    pub async fn instrument<I>(&self, id: I) -> Result<Option<InstrumentMatch>, MarketDataError>
    where
        I: TryInto<InstrumentId>,
        I::Error: Into<InstrumentIdError>,
    {
        let id: InstrumentId = id.try_into().map_err(Into::into)?;
        let source = self
            .find_source(id.source())
            .ok_or_else(|| MarketDataError::UnknownSource(id.source().to_owned()))?;

        let catalog = self.catalog_of(source.as_ref()).await?;
        let Some(instrument) = catalog.find(id.symbol()) else {
            return Ok(None);
        };

        Ok(Some(InstrumentMatch {
            id: InstrumentId::new(catalog.source_id(), &instrument.symbol)?,
            source_name: catalog.source_name_shared(),
            instrument: instrument.clone(),
        }))
    }

    /// Searches every selected source and returns one page, best match
    /// first. Sources that fail are reported in
    /// [`InstrumentPage::failures`] rather than failing the whole search.
    ///
    /// Within one rank tier results are interleaved across sources, so a
    /// venue with thousands of pairs cannot push a smaller one off the page.
    pub async fn instruments(&self, query: impl Into<InstrumentQuery>) -> InstrumentPage {
        let query = query.into();

        let selected: Vec<Arc<dyn MarketDataSource>> = self
            .sources
            .iter()
            .filter(|source| query.accepts_source(source.id()))
            .cloned()
            .collect();

        // Futures are built eagerly: mapping inside the stream trips the
        // borrow checker's higher-ranked closure check under `tokio::spawn`.
        let pending: Vec<_> = selected
            .into_iter()
            .map(|source| self.identified_catalog(source))
            .collect();
        let loaded: Vec<_> = stream::iter(pending)
            .buffer_unordered(SOURCE_CONCURRENCY)
            .collect()
            .await;

        // Rank into small index entries; an `InstrumentMatch` (an id plus a
        // clone of the instrument — seven allocations) is built only for
        // the page actually returned.
        let mut catalogs: Vec<Arc<SourceCatalog>> = Vec::new();
        let mut ranked: Vec<RankedRef> = Vec::new();
        let mut failures = Vec::new();

        for (source_id, result) in loaded {
            let catalog = match result {
                Ok(catalog) => catalog,
                Err(error) => {
                    // One venue being down is not an error for the search:
                    // every other source still contributes, and the caller
                    // sees which one dropped out in `InstrumentPage`.
                    tracing::warn!(
                        source = source_id,
                        %error,
                        "source unavailable; its instruments are absent from this page"
                    );
                    failures.push(SourceFailure { source_id, error });
                    continue;
                }
            };

            let source = catalogs.len();
            // Whether the term matches the venue itself cannot change from
            // one instrument to the next, so it is answered once here.
            let source_matches = query.matches_source(catalog.source_id(), catalog.source_name());
            for (position, instrument) in catalog.instruments().iter().enumerate() {
                let Some(rank) = query.rank(instrument, source_matches) else {
                    continue;
                };
                if !InstrumentId::can_join(catalog.source_id(), &instrument.symbol) {
                    tracing::warn!(
                        source = catalog.source_id(),
                        symbol = instrument.symbol,
                        "instrument cannot be addressed; omitted from results"
                    );
                    continue;
                }
                ranked.push(RankedRef {
                    rank,
                    symbol_len: u16::try_from(instrument.symbol.len()).unwrap_or(u16::MAX),
                    source,
                    position,
                });
            }
            catalogs.push(catalog);
        }

        // Ties are fully broken by symbol, so stability buys nothing. The
        // length is inline so the comparator dereferences only on a full
        // (rank, length) tie.
        ranked.sort_unstable_by(|a, b| {
            a.rank
                .cmp(&b.rank)
                .then_with(|| a.symbol_len.cmp(&b.symbol_len))
                .then_with(|| {
                    let sa = &catalogs[a.source].instruments()[a.position].symbol;
                    let sb = &catalogs[b.source].instruments()[b.position].symbol;
                    sa.cmp(sb)
                })
        });

        let ordered = interleave_by_source(ranked);
        let total_matched = ordered.len();
        let offset = query.offset().min(total_matched);
        let end = query.limit().map_or(total_matched, |limit| {
            offset.saturating_add(limit).min(total_matched)
        });

        let mut matches = Vec::with_capacity(end - offset);
        for entry in &ordered[offset..end] {
            let catalog = &catalogs[entry.source];
            let instrument = &catalog.instruments()[entry.position];
            match InstrumentId::new(catalog.source_id(), &instrument.symbol) {
                Ok(id) => matches.push(InstrumentMatch {
                    id,
                    source_name: catalog.source_name_shared(),
                    instrument: instrument.clone(),
                }),
                // Unreachable while `can_join` above stays in sync with
                // `new`; keep the page honest rather than panic if not.
                Err(error) => tracing::warn!(
                    source = catalog.source_id(),
                    symbol = instrument.symbol,
                    %error,
                    "instrument cannot be addressed; omitted from results"
                ),
            }
        }

        InstrumentPage {
            matches,
            total_matched,
            offset,
            failures,
        }
    }

    /// Drops every in-memory catalog. The next call reloads from disk, or
    /// from the venue if the disk copy is missing or older than the TTL.
    pub fn invalidate(&self) {
        self.catalogs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }

    /// Discards both the in-memory and on-disk catalog for one source and
    /// fetches a fresh one from the venue.
    ///
    /// # Errors
    /// [`MarketDataError::UnknownSource`], or whatever the fetch failed with.
    pub async fn refresh(&self, source_id: &str) -> Result<SourceDetail, MarketDataError> {
        let source = self
            .find_source(source_id)
            .ok_or_else(|| MarketDataError::UnknownSource(source_id.to_owned()))?;

        let storage = Arc::clone(&self.storage);
        let path = instruments_path(source.id());
        run_blocking(move || storage.remove(path)).await?;
        self.catalogs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(source.id());

        let catalog = self.catalog_of(source.as_ref()).await?;
        Ok(describe(&catalog))
    }

    async fn identified_catalog(
        &self,
        source: Arc<dyn MarketDataSource>,
    ) -> (String, Result<Arc<SourceCatalog>, MarketDataError>) {
        let id = source.id().to_owned();
        (id, self.catalog_of(source.as_ref()).await)
    }

    fn find_source(&self, source_id: &str) -> Option<&Arc<dyn MarketDataSource>> {
        self.sources.iter().find(|source| source.id() == source_id)
    }

    fn cell_for(&self, source_id: &str) -> CatalogCell {
        let mut cells = self.catalogs.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(cell) = cells.get(source_id) {
            return Arc::clone(cell);
        }
        let cell = CatalogCell::default();
        cells.insert(source_id.to_owned(), Arc::clone(&cell));
        cell
    }

    async fn catalog_of(
        &self,
        source: &dyn MarketDataSource,
    ) -> Result<Arc<SourceCatalog>, MarketDataError> {
        let cell = self.cell_for(source.id());
        let catalog = cell.get_or_try_init(|| self.load_catalog(source)).await?;
        Ok(Arc::clone(catalog))
    }

    async fn load_catalog(
        &self,
        source: &dyn MarketDataSource,
    ) -> Result<Arc<SourceCatalog>, MarketDataError> {
        let source_id: Arc<str> = Arc::from(source.id());
        let source_name: Arc<str> = Arc::from(source.name());
        let path = instruments_path(&source_id);

        let cached = {
            let storage = Arc::clone(&self.storage);
            let path = path.clone();
            let source_id = Arc::clone(&source_id);
            let ttl = self.cache_ttl;
            run_blocking(move || read_cached(&storage, &path, &source_id, ttl)).await
        };
        let write_back = match cached {
            CacheRead::Hit(snapshot) => {
                tracing::debug!(
                    source = %source_id,
                    count = snapshot.data.len(),
                    "instruments loaded from disk"
                );
                return Ok(Arc::new(SourceCatalog::new(
                    source_id,
                    source_name,
                    snapshot.created_at,
                    snapshot.data,
                )));
            }
            CacheRead::Refetch => true,
            CacheRead::RefetchPreserve => false,
        };

        let instruments = source.instruments().await?;
        assert_unique_symbols(&source_id, &instruments)?;
        let snapshot = Snapshot::new(INSTRUMENTS_SCHEMA_VERSION, instruments);
        tracing::info!(
            source = %source_id,
            count = snapshot.data.len(),
            "instruments fetched from source"
        );

        let snapshot = if write_back {
            let storage = Arc::clone(&self.storage);
            let source_id = Arc::clone(&source_id);
            run_blocking(move || {
                if let Err(error) = storage.write_snapshot(&path, &snapshot) {
                    tracing::warn!(
                        source = %source_id,
                        %error,
                        "failed to cache instruments; returning fetched data anyway"
                    );
                }
                snapshot
            })
            .await
        } else {
            snapshot
        };

        Ok(Arc::new(SourceCatalog::new(
            source_id,
            source_name,
            snapshot.created_at,
            snapshot.data,
        )))
    }
}

impl fmt::Debug for MarketData {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let loaded = self
            .catalogs
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|cell| cell.initialized())
            .count();
        f.debug_struct("MarketData")
            .field(
                "sources",
                &self.sources.iter().map(|s| s.id()).collect::<Vec<_>>(),
            )
            .field("loaded_catalogs", &loaded)
            .field("cache_ttl", &self.cache_ttl)
            .field("storage", &self.storage)
            .finish()
    }
}

fn describe(catalog: &SourceCatalog) -> SourceDetail {
    SourceDetail {
        id: catalog.source_id().to_string(),
        name: catalog.source_name().to_string(),
        instrument_count: catalog.len(),
        tradable_count: catalog
            .instruments()
            .iter()
            .filter(|i| i.status.is_tradable())
            .count(),
        synced_at: catalog.synced_at(),
    }
}

/// Rejects a freshly fetched catalog that reports one symbol twice.
///
/// Measured fact (design record, Part A3): zero duplicates exist across all
/// 51 live sources today, so this must never fire in practice — but the
/// invariant `path_key` and every persisted path rest on is `(source_id,
/// symbol)` uniqueness, and that must be asserted, not assumed. The
/// comparison is case-insensitive, matching [`SourceCatalog`]'s own lookup
/// index, since two symbols differing only in case are exactly the
/// collision that has bitten this project before (Phemex `sOLUSDT` vs
/// `SOLUSDT`).
///
/// # Errors
/// [`MarketDataError::DuplicateSymbol`], naming both source symbols. The
/// caller must not write the snapshot, and must not build a catalog from
/// it: two instruments sharing one symbol would share one on-disk
/// directory and interleave their bars.
fn assert_unique_symbols(
    source_id: &str,
    instruments: &[Instrument],
) -> Result<(), MarketDataError> {
    let mut seen: HashMap<String, &str> = HashMap::with_capacity(instruments.len());
    for instrument in instruments {
        let key = instrument.symbol.to_ascii_uppercase();
        match seen.entry(key) {
            Entry::Occupied(slot) => {
                let first_source_symbol = (*slot.get()).to_owned();
                tracing::error!(
                    source = source_id,
                    symbol = instrument.symbol,
                    first_source_symbol,
                    second_source_symbol = instrument.source_symbol,
                    "duplicate symbol within one source; rejecting the catalog write"
                );
                return Err(MarketDataError::DuplicateSymbol {
                    source_id: source_id.to_owned(),
                    symbol: instrument.symbol.clone(),
                    first_source_symbol,
                    second_source_symbol: instrument.source_symbol.clone(),
                });
            }
            Entry::Vacant(slot) => {
                slot.insert(&instrument.source_symbol);
            }
        }
    }
    Ok(())
}

/// What [`read_cached`] found on disk.
enum CacheRead {
    /// A fresh snapshot in this build's schema: serve it.
    Hit(Snapshot<Vec<Instrument>>),
    /// Nothing usable that belongs to this build: refetch and overwrite.
    Refetch,
    /// A snapshot in a *newer* schema — a newer Senken shares this data
    /// directory. Refetch for this process, but leave the file to its
    /// owner: deleting or overwriting it would make the two builds churn
    /// each other's caches forever.
    RefetchPreserve,
}

/// Reads the on-disk snapshot for one source, treating anything unusable
/// (wrong schema, corrupt, expired) as a miss so the caller refetches.
fn read_cached(storage: &Storage, path: &str, source_id: &str, ttl: Duration) -> CacheRead {
    match storage.read_snapshot(path, INSTRUMENTS_SCHEMA_VERSION) {
        Ok(Some(snapshot)) if snapshot.is_stale(ttl) => {
            tracing::debug!(source = source_id, age = ?snapshot.age(), "instrument cache expired");
            CacheRead::Refetch
        }
        Ok(Some(snapshot)) => CacheRead::Hit(snapshot),
        Ok(None) => CacheRead::Refetch,
        Err(StorageError::SchemaMismatch {
            found, expected, ..
        }) if found < expected => {
            tracing::info!(
                source = source_id,
                found,
                expected,
                "instrument cache has an old schema; refetching from source"
            );
            let _ = storage.remove(path);
            CacheRead::Refetch
        }
        Err(StorageError::SchemaMismatch {
            found, expected, ..
        }) => {
            tracing::warn!(
                source = source_id,
                found,
                expected,
                "instrument cache was written by a newer senken; refetching without touching it"
            );
            CacheRead::RefetchPreserve
        }
        Err(error) => {
            tracing::warn!(
                source = source_id,
                %error,
                "unreadable instrument cache; refetching from source"
            );
            let _ = storage.remove(path);
            CacheRead::Refetch
        }
    }
}

/// Runs synchronous storage work off the async executor.
///
/// A panic inside `f` is propagated; the only other way the task can fail
/// is runtime shutdown, at which point there is nothing left to return to.
async fn run_blocking<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(f).await {
        Ok(value) => value,
        Err(error) => match error.try_into_panic() {
            Ok(payload) => std::panic::resume_unwind(payload),
            Err(error) => panic!("blocking storage task cancelled: {error}"),
        },
    }
}

/// One ranked hit as indices into the search's loaded catalogs. Search
/// sorts and interleaves these small `Copy` entries and materialises an
/// [`InstrumentMatch`] only for the page it returns.
#[derive(Clone, Copy)]
struct RankedRef {
    rank: MatchRank,
    /// Symbol length (saturated), inline so rank ties rarely dereference.
    symbol_len: u16,
    /// Index into the search's catalog list.
    source: usize,
    /// Index into that catalog's instruments.
    position: usize,
}

/// Round-robins each rank tier across sources so no single venue can
/// monopolise a page. Tiers never mix: every hit of a better rank still
/// precedes every hit of a worse one.
fn interleave_by_source(ranked: Vec<RankedRef>) -> Vec<RankedRef> {
    let mut ordered = Vec::with_capacity(ranked.len());
    let mut tier: Vec<VecDeque<RankedRef>> = Vec::new();
    let mut current: Option<MatchRank> = None;

    for item in ranked {
        if current != Some(item.rank) {
            drain_round_robin(&mut tier, &mut ordered);
            current = Some(item.rank);
        }
        let queue = tier
            .iter_mut()
            .find(|queue| queue.front().is_some_and(|head| head.source == item.source));
        match queue {
            Some(queue) => queue.push_back(item),
            None => tier.push(VecDeque::from([item])),
        }
    }
    drain_round_robin(&mut tier, &mut ordered);

    ordered
}

fn drain_round_robin(tier: &mut Vec<VecDeque<RankedRef>>, ordered: &mut Vec<RankedRef>) {
    loop {
        let mut emitted = false;
        for queue in tier.iter_mut() {
            if let Some(item) = queue.pop_front() {
                ordered.push(item);
                emitted = true;
            }
        }
        if !emitted {
            break;
        }
    }
    tier.clear();
}

#[cfg(test)]
mod tests {
    use super::{
        INSTRUMENTS_SCHEMA_VERSION, InstrumentMatch, InstrumentPage, MarketData, MarketDataError,
    };
    use crate::id::InstrumentId;
    use crate::instrument::{Instrument, InstrumentStatus};
    use crate::paths::instruments_path;
    use crate::query::InstrumentQuery;
    use crate::source::{MarketDataSource, SourceError};
    use async_trait::async_trait;
    use senken_storage::{Snapshot, Storage};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use tempfile::TempDir;

    struct FakeSource {
        id: &'static str,
        name: &'static str,
        symbols: Vec<&'static str>,
        fetches: Arc<AtomicUsize>,
        failures_left: AtomicUsize,
        delay: Duration,
    }

    #[async_trait]
    impl MarketDataSource for FakeSource {
        fn id(&self) -> &str {
            self.id
        }

        fn name(&self) -> &str {
            self.name
        }

        async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
            self.fetches.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            if self
                .failures_left
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                return Err(SourceError::http(503, "try later"));
            }
            Ok(self
                .symbols
                .iter()
                .map(|symbol| {
                    Instrument::spot(*symbol, *symbol, symbol.trim_end_matches("USDT"), "USDT")
                        .with_name(*symbol)
                        .with_status(InstrumentStatus::Trading)
                        .with_price_increment((2, 1))
                        .with_qty_increment((2, 1))
                })
                .collect())
        }
    }

    fn fake(id: &'static str, name: &'static str, symbols: Vec<&'static str>) -> FakeSource {
        FakeSource {
            id,
            name,
            symbols,
            fetches: Arc::new(AtomicUsize::new(0)),
            failures_left: AtomicUsize::new(0),
            delay: Duration::ZERO,
        }
    }

    fn market_data(sources: Vec<FakeSource>) -> (TempDir, MarketData) {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path());
        storage.init().unwrap();

        let mut md = MarketData::new(Arc::new(storage));
        for source in sources {
            md.register_source(Arc::new(source)).unwrap();
        }
        (dir, md)
    }

    fn symbols_of(outcome: &InstrumentPage) -> Vec<(&str, &str)> {
        outcome
            .matches
            .iter()
            .map(|m| (m.source_id(), m.instrument.symbol.as_str()))
            .collect()
    }

    #[test]
    fn instruments_are_grouped_per_source() {
        assert_eq!(
            instruments_path("binance-spot"),
            "sources/binance-spot/instruments.json"
        );
    }

    #[test]
    fn registration_rejects_bad_and_duplicate_ids() {
        let (_dir, mut md) = market_data(vec![fake("okx", "OKX", vec![])]);
        assert!(matches!(
            md.register_source(Arc::new(fake("OKX", "OKX", vec![]))),
            Err(MarketDataError::InvalidSourceId(_))
        ));
        assert!(matches!(
            md.register_source(Arc::new(fake("okx", "OKX again", vec![]))),
            Err(MarketDataError::DuplicateSource(_))
        ));
        assert_eq!(md.sources().len(), 1);
    }

    #[tokio::test]
    async fn listing_sources_touches_neither_disk_nor_network() {
        let source = fake("fake", "Fake Venue", vec!["AUSDT"]);
        let fetches = Arc::clone(&source.fetches);
        let (_dir, md) = market_data(vec![source]);

        let sources = md.sources();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].id, "fake");
        assert_eq!(fetches.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_catalog_is_loaded_once_and_reused() {
        let source = fake("fake", "Fake Venue", vec!["AUSDT", "BUSDT"]);
        let fetches = Arc::clone(&source.fetches);
        let (_dir, md) = market_data(vec![source]);

        md.instruments("a").await;
        md.instruments("b").await;
        md.source_detail("fake").await.unwrap();
        assert_eq!(fetches.load(Ordering::SeqCst), 1);

        md.invalidate();
        md.instruments("a").await;
        assert_eq!(
            fetches.load(Ordering::SeqCst),
            1,
            "reload must come from disk, not the source"
        );
    }

    #[tokio::test]
    async fn an_expired_cache_is_refetched() {
        let source = fake("fake", "Fake Venue", vec!["AUSDT"]);
        let fetches = Arc::clone(&source.fetches);
        let (_dir, md) = market_data(vec![source]);
        let md = md.with_cache_ttl(Duration::ZERO);

        md.instruments("a").await;
        md.invalidate();
        md.instruments("a").await;
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn refresh_bypasses_both_caches() {
        let source = fake("fake", "Fake Venue", vec!["AUSDT"]);
        let fetches = Arc::clone(&source.fetches);
        let (_dir, md) = market_data(vec![source]);

        md.instruments("a").await;
        let detail = md.refresh("fake").await.unwrap();
        assert_eq!(detail.instrument_count, 1);
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
        assert!(md.refresh("nope").await.is_err());
    }

    #[tokio::test]
    async fn an_older_schema_on_disk_is_replaced() {
        let source = fake("fake", "Fake Venue", vec!["AUSDT"]);
        let (dir, md) = market_data(vec![source]);
        let storage = Storage::new(dir.path());
        let path = instruments_path("fake");
        storage
            .write_snapshot(
                &path,
                &Snapshot::new(INSTRUMENTS_SCHEMA_VERSION - 1, Vec::<Instrument>::new()),
            )
            .unwrap();

        let page = md.instruments("a").await;
        assert_eq!(page.matches.len(), 1, "old cache must not be served");

        let replaced: Snapshot<Vec<Instrument>> = storage
            .read_snapshot(&path, INSTRUMENTS_SCHEMA_VERSION)
            .unwrap()
            .expect("the refetch must be cached in the current schema");
        assert_eq!(replaced.data.len(), 1);
    }

    #[tokio::test]
    async fn a_newer_schema_on_disk_is_preserved() {
        let source = fake("fake", "Fake Venue", vec!["AUSDT"]);
        let fetches = Arc::clone(&source.fetches);
        let (dir, md) = market_data(vec![source]);
        let storage = Storage::new(dir.path());
        let path = instruments_path("fake");
        storage
            .write_snapshot(
                &path,
                &Snapshot::new(INSTRUMENTS_SCHEMA_VERSION + 1, Vec::<Instrument>::new()),
            )
            .unwrap();

        let page = md.instruments("a").await;
        assert_eq!(page.matches.len(), 1, "the venue is asked instead");
        assert_eq!(fetches.load(Ordering::SeqCst), 1);

        let preserved = storage
            .read_snapshot::<Vec<Instrument>>(&path, INSTRUMENTS_SCHEMA_VERSION + 1)
            .unwrap();
        assert!(
            preserved.is_some(),
            "a snapshot from a newer senken must be left in place"
        );
    }

    #[tokio::test]
    async fn concurrent_first_calls_fetch_only_once() {
        let source = fake("fake", "Fake Venue", vec!["AUSDT"]);
        let fetches = Arc::clone(&source.fetches);
        let (_dir, md) = market_data(vec![source]);
        let md = Arc::new(md);

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let md = Arc::clone(&md);
            tasks.push(tokio::spawn(async move { md.instruments("a").await }));
        }
        for task in tasks {
            task.await.unwrap();
        }

        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn sources_load_concurrently_not_one_after_another() {
        let delay = Duration::from_millis(200);
        let sources = ["a", "b", "c", "d"]
            .map(|id| FakeSource {
                delay,
                ..fake(id, "Slow Venue", vec!["BTCUSDT"])
            })
            .into_iter()
            .collect();
        let (_dir, md) = market_data(sources);

        let start = Instant::now();
        let page = md.instruments("btc").await;
        let elapsed = start.elapsed();

        assert_eq!(page.total_matched, 4);
        assert!(
            elapsed < delay * 2,
            "four {delay:?} loads took {elapsed:?}; they ran serially"
        );
    }

    #[tokio::test]
    async fn a_failed_load_is_retried_on_the_next_call() {
        let source = FakeSource {
            failures_left: AtomicUsize::new(1),
            ..fake("flaky", "Flaky Venue", vec!["AUSDT"])
        };
        let fetches = Arc::clone(&source.fetches);
        let (_dir, md) = market_data(vec![source]);

        let first = md.instruments("a").await;
        assert!(!first.is_complete());
        assert_eq!(first.failures[0].source_id, "flaky");
        assert!(first.matches.is_empty());

        let second = md.instruments("a").await;
        assert!(second.is_complete());
        assert_eq!(second.matches.len(), 1);
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn a_duplicate_symbol_within_one_source_is_rejected_not_silently_kept() {
        // Case differs only in the leading letter, exactly the shape of the
        // real collision this rule exists for (Phemex `sOLUSDT` vs
        // `SOLUSDT`) — never keep one and drop the other, because the two
        // would then share one on-disk directory.
        let source = fake("dup", "Dup Venue", vec!["SOLUSDT", "solusdt"]);
        let (_dir, md) = market_data(vec![source]);

        let page = md.instruments("sol").await;
        assert!(!page.is_complete(), "a colliding source must fail to load");
        assert!(page.matches.is_empty());
        assert!(matches!(
            page.failures[0].error,
            MarketDataError::DuplicateSymbol { .. }
        ));
    }

    #[tokio::test]
    async fn an_instrument_is_addressable_by_its_id() {
        let (_dir, md) = market_data(vec![fake("okx", "OKX", vec!["BTCUSDT", "ETH-USDT"])]);

        let id = InstrumentId::parse("okx:BTCUSDT").unwrap();
        let found = md.instrument(&id).await.unwrap().unwrap();
        assert_eq!(found.instrument.symbol, "BTCUSDT");
        assert_eq!(found.id.as_str(), "okx:BTCUSDT");
        assert_eq!(found.source_id(), "okx");
        assert_eq!(&*found.source_name, "OKX");

        // the same lookup from an owned id, a &str, and a String
        assert_eq!(md.instrument(id.clone()).await.unwrap().unwrap().id, id);
        assert_eq!(md.instrument("okx:BTCUSDT").await.unwrap().unwrap().id, id);
        assert_eq!(
            md.instrument(String::from("okx:BTCUSDT"))
                .await
                .unwrap()
                .unwrap()
                .id,
            id
        );
        assert!(matches!(
            md.instrument("not-an-id").await,
            Err(MarketDataError::Id(_))
        ));

        let lowercase = InstrumentId::parse("okx:btcusdt").unwrap();
        assert!(md.instrument(&lowercase).await.unwrap().is_some());

        let missing = InstrumentId::parse("okx:SOLUSDT").unwrap();
        assert!(md.instrument(&missing).await.unwrap().is_none());

        assert!(matches!(
            md.instrument("nope:BTCUSDT").await,
            Err(MarketDataError::UnknownSource(_))
        ));
    }

    #[tokio::test]
    async fn source_detail_reports_catalog_statistics() {
        let (_dir, md) = market_data(vec![fake("okx", "OKX", vec!["A", "B", "C"])]);

        let detail = md.source_detail("okx").await.unwrap();
        assert_eq!(detail.id, "okx");
        assert_eq!(detail.instrument_count, 3);
        assert_eq!(detail.tradable_count, 3);
        assert!(md.source_detail("missing").await.is_err());
    }

    #[tokio::test]
    async fn search_ranks_exact_before_prefix_before_substring() {
        let (_dir, md) = market_data(vec![fake(
            "fake",
            "Fake Venue",
            vec!["WBTCUSDT", "BTCSTUSDT", "BTCUSDT", "ETHUSDT"],
        )]);
        let outcome = md.instruments("btc").await;

        let symbols: Vec<&str> = outcome
            .matches
            .iter()
            .map(|m| m.instrument.symbol.as_str())
            .collect();
        assert_eq!(symbols, ["BTCUSDT", "BTCSTUSDT", "WBTCUSDT"]);
        assert!(outcome.is_complete());
    }

    #[tokio::test]
    async fn pagination_walks_the_whole_result_set() {
        let (_dir, md) = market_data(vec![fake(
            "fake",
            "Fake Venue",
            vec!["AUSDT", "BUSDT", "CUSDT", "DUSDT", "EUSDT"],
        )]);

        let first = md.instruments(InstrumentQuery::all().with_page(0, 2)).await;
        assert_eq!(symbols_of(&first), [("fake", "AUSDT"), ("fake", "BUSDT")]);
        assert_eq!(first.total_matched, 5);
        assert!(first.has_more());

        let second = md.instruments(InstrumentQuery::all().with_page(1, 2)).await;
        assert_eq!(symbols_of(&second), [("fake", "CUSDT"), ("fake", "DUSDT")]);

        let last = md.instruments(InstrumentQuery::all().with_page(2, 2)).await;
        assert_eq!(symbols_of(&last), [("fake", "EUSDT")]);
        assert_eq!(last.offset, 4);
        assert!(!last.has_more());

        let past_end = md.instruments(InstrumentQuery::all().with_page(9, 2)).await;
        assert!(past_end.matches.is_empty());
        assert_eq!(past_end.total_matched, 5);
        assert!(!past_end.has_more());
    }

    #[tokio::test]
    async fn a_loud_source_cannot_starve_the_others() {
        let (_dir, md) = market_data(vec![
            fake(
                "loud",
                "Loud Venue",
                vec!["BTCUSDT", "BTCUSDC", "BTCTRY", "BTCEUR", "BTCGBP"],
            ),
            fake("quiet", "Quiet Venue", vec!["BTCUSDT"]),
        ]);

        let outcome = md
            .instruments(InstrumentQuery::new("btc").with_limit(3))
            .await;

        let sources: Vec<&str> = outcome
            .matches
            .iter()
            .map(InstrumentMatch::source_id)
            .collect();
        assert!(
            sources.contains(&"quiet"),
            "quiet source was starved: {sources:?}"
        );
        assert_eq!(outcome.total_matched, 6);
    }

    #[tokio::test]
    async fn interleaving_never_crosses_rank_tiers() {
        let (_dir, md) = market_data(vec![
            fake("a", "A Venue", vec!["BTCUSDT", "BTCUSDC"]),
            fake("b", "B Venue", vec!["WBTCUSDT"]),
        ]);

        let outcome = md.instruments("btc").await;
        assert_eq!(
            symbols_of(&outcome),
            [("a", "BTCUSDT"), ("a", "BTCUSDC"), ("b", "WBTCUSDT")],
            "a substring match must never outrank an exact-base match"
        );
    }

    #[tokio::test]
    async fn a_source_prefix_narrows_the_search() {
        let (_dir, md) = market_data(vec![
            fake("binance-spot", "Binance Spot", vec!["XAUTUSDT"]),
            fake("okx", "OKX", vec!["XAUT-USDT"]),
        ]);

        assert_eq!(
            symbols_of(&md.instruments("okx:xaut").await),
            [("okx", "XAUT-USDT")]
        );
        assert_eq!(
            symbols_of(&md.instruments("binance:xaut").await),
            [("binance-spot", "XAUTUSDT")]
        );
        assert_eq!(md.instruments("xaut").await.total_matched, 2);
        assert_eq!(md.instruments("okx:").await.total_matched, 1);
        assert!(md.instruments("nope:xaut").await.matches.is_empty());
    }

    #[tokio::test]
    async fn a_query_can_match_the_source_name() {
        let (_dir, md) = market_data(vec![fake("fake", "Fake Venue", vec!["AUSDT", "BUSDT"])]);
        assert_eq!(md.instruments("fake venue").await.matches.len(), 2);
    }

    #[tokio::test]
    async fn debug_output_is_useful() {
        let (_dir, md) = market_data(vec![fake("okx", "OKX", vec!["AUSDT"])]);
        assert!(format!("{md:?}").contains("loaded_catalogs: 0"));
        md.instruments("a").await;
        assert!(format!("{md:?}").contains("loaded_catalogs: 1"));
    }
}
