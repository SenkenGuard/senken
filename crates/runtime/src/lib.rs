//! The Senken runtime.
//!
//! Where the domain crates (`senken-marketdata`, `senken-storage`, …) are
//! each usable alone, this crate is the opposite: it exists only to assemble
//! them, plus any number of plugins, into one running application.
//!
//! ```rust,no_run
//! use senken_runtime::Runtime;
//!
//! # fn main() -> Result<(), senken_runtime::RuntimeError> {
//! let runtime = Runtime::builder()
//!     .data_dir("/var/lib/senken")
//!     // .plugin(senken_plugin_binance::BinancePlugin)
//!     .build()?;
//!
//! let sources = runtime.marketdata().sources();
//! // Bars, wired the same way — one loader per registered
//! // `BarSource`, e.g. `okx-spot`:
//! if let Some(_loader) = runtime.series().loader("okx-spot") {
//!     // loader.plan(..) / .ensure(..) — see `senken_loader::SeriesLoader`.
//! }
//! runtime.shutdown()?;
//! # Ok(()) }
//! ```

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use senken_identity::IdentityStore;
use senken_marketdata::MarketData;
use senken_marketdata::book::BookSource;
use senken_plugin::{
    ActivationContext, BarSource, Plugin, PluginError, PluginManifest, reconcile_plugin_permissions,
};
use senken_storage::Storage;
use senken_store::Store;
use senken_subscription::FeedSource;

/// Error types.
pub mod error;
/// Bar-fetching services: [`SeriesData`], `Runtime::series()`.
mod series;

pub use crate::error::RuntimeError;
pub use crate::series::SeriesData;

/// Where data lives when the builder is given no other location.
pub const DEFAULT_DATA_DIR: &str = ".data";

/// An activated plugin.
#[derive(Debug)]
pub struct PluginRecord {
    plugin: Box<dyn Plugin>,
    manifest: PluginManifest,
    activated_at: SystemTime,
}

impl PluginRecord {
    /// The plugin itself.
    #[must_use]
    pub fn plugin(&self) -> &dyn Plugin {
        self.plugin.as_ref()
    }

    /// The manifest captured at activation.
    #[must_use]
    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// When activation succeeded.
    #[must_use]
    pub fn activated_at(&self) -> SystemTime {
        self.activated_at
    }
}

/// Configures and starts a [`Runtime`].
#[derive(Debug)]
pub struct RuntimeBuilder {
    storage: Storage,
    cache_ttl: Option<Duration>,
    plugins: Vec<Box<dyn Plugin>>,
    identity: Option<Arc<IdentityStore>>,
}

impl RuntimeBuilder {
    fn new() -> Self {
        Self {
            storage: Storage::new(DEFAULT_DATA_DIR),
            cache_ttl: None,
            plugins: Vec::new(),
            identity: None,
        }
    }

    /// Stores all data under `path`. Defaults to [`DEFAULT_DATA_DIR`].
    #[must_use]
    pub fn data_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.storage = Storage::new(path);
        self
    }

    /// Uses a pre-configured [`Storage`] instead of a plain data directory.
    #[must_use]
    pub fn storage(mut self, storage: Storage) -> Self {
        self.storage = storage;
        self
    }

    /// How long cached market data catalogs are trusted. Defaults to
    /// [`senken_marketdata::DEFAULT_CACHE_TTL`].
    #[must_use]
    pub fn marketdata_cache_ttl(mut self, ttl: Duration) -> Self {
        self.cache_ttl = Some(ttl);
        self
    }

    /// Adds a plugin. Plugins activate in the order they are added.
    #[must_use]
    pub fn plugin(mut self, plugin: impl Plugin + 'static) -> Self {
        self.plugins.push(Box::new(plugin));
        self
    }

    /// Uses this identity store to reconcile plugin permissions at startup.
    #[must_use]
    pub fn identity_store(mut self, identity: Arc<IdentityStore>) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Prepares storage and activates every plugin.
    ///
    /// Activation is all-or-nothing: the first plugin that fails aborts
    /// startup, and every plugin activated before it is deactivated again.
    /// A runtime that starts has every plugin it was given.
    ///
    /// # Errors
    /// See [`RuntimeError`].
    pub fn build(self) -> Result<Runtime, RuntimeError> {
        self.storage
            .init()
            .map_err(|source| RuntimeError::StorageInit { source })?;

        let storage = Arc::new(self.storage);
        let mut marketdata = MarketData::new(Arc::clone(&storage));
        if let Some(ttl) = self.cache_ttl {
            marketdata = marketdata.with_cache_ttl(ttl);
        }

        let mut records: Vec<PluginRecord> = Vec::with_capacity(self.plugins.len());
        let mut seen = HashSet::with_capacity(self.plugins.len());
        let mut registered = Registrations::default();
        // One context for the whole run, so resources it caches (the shared
        // HTTP client) are shared by every plugin.
        let mut context = ActivationContext::new();

        for plugin in self.plugins {
            let manifest = plugin.manifest();
            if let Err(error) = activate(
                &*plugin,
                &manifest,
                &mut context,
                &mut marketdata,
                &mut registered,
                &mut seen,
                self.identity.as_deref(),
            ) {
                // The activation failure is the one worth returning; unwind
                // problems are already logged by `deactivate_all`.
                let _ = deactivate_all(&mut records);
                return Err(error);
            }
            tracing::info!(
                plugin = manifest.id,
                version = manifest.version,
                "plugin active"
            );
            records.push(PluginRecord {
                plugin,
                manifest,
                activated_at: SystemTime::now(),
            });
        }

        let marketdata = Arc::new(marketdata);

        // Rooted at the same data directory as everything else;
        // `senken-store`'s `sources/{id}/instruments/{KEY}/bars/...` layout
        // lives alongside `senken-marketdata`'s own
        // `sources/{id}/instruments.json` under one `.data` tree.
        let series_store = Store::new(storage.data_dir());
        series_store
            .init()
            .map_err(|source| RuntimeError::SeriesStoreInit { source })?;
        let series = SeriesData::build(&series_store, &marketdata, registered.bars);

        Ok(Runtime {
            storage,
            plugins: records,
            marketdata,
            series,
            series_store,
            book_sources: registered
                .book
                .into_iter()
                .map(|source| (source.source_id().to_owned(), source))
                .collect(),
            feed_sources: registered.feeds,
        })
    }
}

/// Everything one activation pass accumulates across plugins, bundled so
/// [`activate`] takes one argument for it rather than one per capability —
/// a count that grows every time the plugin contract does.
#[derive(Default)]
struct Registrations {
    bars: Vec<Arc<dyn BarSource>>,
    book: Vec<Arc<dyn BookSource>>,
    feeds: Vec<Arc<dyn FeedSource>>,
}

fn activate(
    plugin: &dyn Plugin,
    manifest: &PluginManifest,
    context: &mut ActivationContext,
    marketdata: &mut MarketData,
    registered: &mut Registrations,
    seen: &mut HashSet<String>,
    identity: Option<&IdentityStore>,
) -> Result<(), RuntimeError> {
    if !seen.insert(manifest.id.clone()) {
        return Err(RuntimeError::DuplicatePlugin(manifest.id.clone()));
    }

    manifest
        .validate_permissions()
        .map_err(PluginError::other)
        .map_err(|source| RuntimeError::PluginActivation {
            plugin: manifest.id.clone(),
            source,
        })?;
    let namespace = manifest
        .permission_namespace()
        .map_err(PluginError::other)
        .map_err(|source| RuntimeError::PluginActivation {
            plugin: manifest.id.clone(),
            source,
        })?;
    context.bind_permission_namespace(namespace);

    if let Err(source) = plugin.activate(context) {
        // A plugin that failed part-way may have registered something
        // first; it must not leak into the next plugin's activation.
        drop(context.take_marketdata_sources());
        drop(context.take_bar_sources());
        return Err(RuntimeError::PluginActivation {
            plugin: manifest.id.clone(),
            source,
        });
    }

    let mut permissions = manifest.permissions.clone();
    permissions.extend(context.take_plugin_permissions());
    if let Some(identity) = identity {
        let previous = identity
            .load_plugin_permissions(&manifest.id)
            .map_err(PluginError::other)
            .map_err(|source| RuntimeError::PluginActivation {
                plugin: manifest.id.clone(),
                source,
            })?;
        let reconciled = reconcile_plugin_permissions(&previous, &permissions);
        identity
            .save_plugin_permissions(&manifest.id, &reconciled)
            .map_err(PluginError::other)
            .map_err(|source| RuntimeError::PluginActivation {
                plugin: manifest.id.clone(),
                source,
            })?;
    }

    for source in context.take_marketdata_sources() {
        tracing::info!(
            plugin = manifest.id,
            source = source.id(),
            "registering marketdata source"
        );
        marketdata
            .register_source(source)
            .map_err(|source| RuntimeError::SourceRegistration {
                plugin: manifest.id.clone(),
                source,
            })?;
    }

    for source in context.take_bar_sources() {
        tracing::info!(
            plugin = manifest.id,
            source = source.source_id(),
            "registering bar source"
        );
        registered.bars.push(source);
    }
    for source in context.take_book_sources() {
        tracing::info!(
            plugin = manifest.id,
            source = source.source_id(),
            "registering book source"
        );
        registered.book.push(source);
    }
    for source in context.take_feed_sources() {
        tracing::info!(
            plugin = manifest.id,
            sources = ?source.source_ids(),
            "registering live feed"
        );
        registered.feeds.push(source);
    }
    Ok(())
}

/// Deactivates in reverse activation order, continuing past failures.
/// Returns the first failure, if any.
fn deactivate_all(records: &mut Vec<PluginRecord>) -> Result<(), RuntimeError> {
    let mut first_error = None;
    while let Some(record) = records.pop() {
        if let Err(source) = record.plugin.deactivate() {
            tracing::error!(plugin = record.manifest.id, %source, "plugin failed to deactivate");
            first_error.get_or_insert(RuntimeError::PluginDeactivation {
                plugin: record.manifest.id.clone(),
                source,
            });
        } else {
            tracing::info!(plugin = record.manifest.id, "plugin deactivated");
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// A running Senken application: storage, every domain service, and the
/// plugins that populate them.
///
/// Dropping a runtime deactivates any remaining plugins on a best-effort
/// basis, logging failures. Call [`shutdown`](Self::shutdown) instead when
/// a deactivation failure should be observed.
#[derive(Debug)]
pub struct Runtime {
    storage: Arc<Storage>,
    plugins: Vec<PluginRecord>,
    marketdata: Arc<MarketData>,
    series: SeriesData,
    /// The same [`Store`] every [`SeriesData`] loader was built against —
    /// kept here too so a caller that needs to inspect or reclaim what is
    /// on disk (a Settings "storage usage" report, say) reuses this
    /// instance rather than constructing a second `Store` pointed at the
    /// same directory. Cheap to clone (a path plus an `Arc`'d lock table).
    series_store: Store,
    /// Every order-book source the active plugins registered, keyed by the
    /// source id each one states it serves.
    ///
    /// Owned here rather than assembled by whatever serves HTTP, for the
    /// same reason bar sources are: registration is the capability
    /// declaration, and a headless caller must be able to read it without
    /// an HTTP layer to inherit it from. This replaced a hardcoded map in
    /// `senken-api` that could only ever hold one venue.
    book_sources: HashMap<String, Arc<dyn BookSource>>,
    /// Every live feed the active plugins registered. Not yet connected:
    /// a [`FeedSource`] is the means to build a protocol once a catalog
    /// exists, and whoever runs live data decides when that happens.
    feed_sources: Vec<Arc<dyn FeedSource>>,
}

impl Runtime {
    /// The order-book source registered for `source_id`, if any.
    ///
    /// `None` is a real answer — the venue serves no depth in this build —
    /// not a lookup failure, and a caller reporting capabilities reads it
    /// as such.
    #[must_use]
    pub fn book_source(&self, source_id: &str) -> Option<&Arc<dyn BookSource>> {
        self.book_sources.get(source_id)
    }

    /// Whether any plugin registered depth for `source_id`.
    #[must_use]
    pub fn has_book_source(&self, source_id: &str) -> bool {
        self.book_sources.contains_key(source_id)
    }

    /// Every live feed the active plugins registered.
    #[must_use]
    pub fn feed_sources(&self) -> &[Arc<dyn FeedSource>] {
        &self.feed_sources
    }
    /// Starts configuring a runtime.
    #[must_use]
    pub fn builder() -> RuntimeBuilder {
        RuntimeBuilder::new()
    }

    /// The data directory everything persists into.
    #[must_use]
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    /// Every plugin, in activation order.
    #[must_use]
    pub fn plugins(&self) -> &[PluginRecord] {
        &self.plugins
    }

    /// The market data service.
    #[must_use]
    pub fn marketdata(&self) -> &MarketData {
        &self.marketdata
    }

    /// The bar-fetching services: one [`senken_loader::SeriesLoader`]
    /// per registered [`senken_plugin::BarSource`]. See [`SeriesData`]'s own
    /// docs for why this mirrors [`Self::marketdata`] in spirit rather than
    /// in exact shape.
    #[must_use]
    pub fn series(&self) -> &SeriesData {
        &self.series
    }

    /// The [`Store`] every registered source's [`senken_loader::SeriesLoader`]
    /// reads and writes through — the same instance, not a second one
    /// pointed at the same directory, so a caller inspecting or reclaiming
    /// on-disk usage never races a loader's own compaction lock.
    #[must_use]
    pub fn store(&self) -> &Store {
        &self.series_store
    }

    /// Deactivates every plugin in reverse order and consumes the runtime.
    ///
    /// # Errors
    /// The first [`RuntimeError::PluginDeactivation`] encountered; the
    /// remaining plugins are still deactivated.
    pub fn shutdown(mut self) -> Result<(), RuntimeError> {
        deactivate_all(&mut self.plugins)
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // After an explicit `shutdown` the list is already empty. Failures
        // are logged inside `deactivate_all`; a drop site cannot handle them.
        let _ = deactivate_all(&mut self.plugins);
    }
}

#[cfg(test)]
mod tests {
    use super::{Runtime, RuntimeError};
    use senken_acl::PluginPermissionName;
    use senken_identity::IdentityStore;
    use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    struct Recording {
        id: &'static str,
        fail_activation: bool,
        deactivations: Arc<AtomicUsize>,
    }

    impl Plugin for Recording {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.id.to_string(),
                name: self.id.to_string(),
                version: "0".into(),
                description: String::new(),
                permissions: Vec::new(),
            }
        }

        fn activate(&self, _: &mut ActivationContext) -> Result<(), PluginError> {
            if self.fail_activation {
                Err(PluginError::msg("nope"))
            } else {
                Ok(())
            }
        }

        fn deactivate(&self) -> Result<(), PluginError> {
            self.deactivations.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn plugin(id: &'static str, fail: bool, counter: &Arc<AtomicUsize>) -> Recording {
        Recording {
            id,
            fail_activation: fail,
            deactivations: Arc::clone(counter),
        }
    }

    #[test]
    fn a_failing_plugin_aborts_startup_and_unwinds_the_others() {
        let dir = TempDir::new().unwrap();
        let deactivations = Arc::new(AtomicUsize::new(0));
        let err = Runtime::builder()
            .data_dir(dir.path())
            .plugin(plugin("first", false, &deactivations))
            .plugin(plugin("second", true, &deactivations))
            .plugin(plugin("third", false, &deactivations))
            .build()
            .unwrap_err();

        assert!(
            matches!(err, RuntimeError::PluginActivation { ref plugin, .. } if plugin == "second")
        );
        assert_eq!(
            deactivations.load(Ordering::SeqCst),
            1,
            "only `first` was active"
        );
    }

    #[test]
    fn duplicate_plugin_ids_are_rejected() {
        let dir = TempDir::new().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let err = Runtime::builder()
            .data_dir(dir.path())
            .plugin(plugin("same", false, &counter))
            .plugin(plugin("same", false, &counter))
            .build()
            .unwrap_err();
        assert!(matches!(err, RuntimeError::DuplicatePlugin(ref id) if id == "same"));
    }

    #[test]
    fn startup_reconciles_a_plugins_declared_permissions() {
        struct PermissionPlugin;

        impl Plugin for PermissionPlugin {
            fn manifest(&self) -> PluginManifest {
                PluginManifest {
                    id: "test-plugin".to_owned(),
                    name: "Test plugin".to_owned(),
                    version: "0".to_owned(),
                    description: String::new(),
                    permissions: Vec::new(),
                }
            }

            fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
                context
                    .register_plugin_permission(
                        PluginPermissionName::parse("test-plugin.chart:view").unwrap(),
                    )
                    .map_err(PluginError::other)?;
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        let identity = Arc::new(IdentityStore::open(dir.path().join("accounts.db")).unwrap());
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .identity_store(Arc::clone(&identity))
            .plugin(PermissionPlugin)
            .build()
            .unwrap();

        let records = identity.load_plugin_permissions("test-plugin").unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].name().as_str(), "test-plugin.chart:view");
        runtime.shutdown().unwrap();
    }

    #[test]
    fn every_plugins_sources_reach_the_registry() {
        struct VenueStub(&'static str);

        #[async_trait::async_trait]
        impl senken_marketdata::MarketDataSource for VenueStub {
            fn id(&self) -> &str {
                self.0
            }

            fn name(&self) -> &str {
                self.0
            }

            async fn instruments(
                &self,
            ) -> Result<Vec<senken_marketdata::Instrument>, senken_marketdata::SourceError>
            {
                Ok(Vec::new())
            }
        }

        struct SourcePlugin(&'static str);

        impl Plugin for SourcePlugin {
            fn manifest(&self) -> PluginManifest {
                PluginManifest {
                    id: self.0.to_string(),
                    name: self.0.to_string(),
                    version: "0".into(),
                    description: String::new(),
                    permissions: Vec::new(),
                }
            }

            fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
                context.register_marketdata_source(Arc::new(VenueStub(self.0)));
                Ok(())
            }
        }

        let dir = TempDir::new().unwrap();
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .plugin(SourcePlugin("venue-a"))
            .plugin(SourcePlugin("venue-b"))
            .build()
            .unwrap();

        let ids: Vec<String> = runtime
            .marketdata()
            .sources()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert_eq!(ids, ["venue-a", "venue-b"]);
        runtime.shutdown().unwrap();
    }

    #[test]
    fn dropping_without_shutdown_still_deactivates() {
        let dir = TempDir::new().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .plugin(plugin("a", false, &counter))
            .plugin(plugin("b", false, &counter))
            .build()
            .unwrap();

        drop(runtime);
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn shutdown_deactivates_every_plugin() {
        let dir = TempDir::new().unwrap();
        let counter = Arc::new(AtomicUsize::new(0));
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .plugin(plugin("a", false, &counter))
            .plugin(plugin("b", false, &counter))
            .build()
            .unwrap();

        assert_eq!(runtime.plugins().len(), 2);
        assert_eq!(runtime.plugins()[0].manifest().id, "a");
        assert!(runtime.storage().data_dir().is_dir());
        runtime.shutdown().unwrap();
        assert_eq!(counter.load(Ordering::SeqCst), 2);
    }
}
