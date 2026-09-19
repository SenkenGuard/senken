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
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::{Duration, SystemTime};

use senken_identity::IdentityStore;
use senken_marketdata::MarketData;
use senken_marketdata::book::BookSource;
use senken_plugin::widget_package::{PackageStatus, WidgetPackageStore};
use senken_plugin::{
    ActivationContext, BarSource, ContributionKind, Plugin, PluginError, PluginManifest,
    reconcile_plugin_permissions,
};
use senken_storage::Storage;
use senken_store::Store;
use senken_subscription::FeedSource;
use senken_trade::TradeEngine;

/// Error types.
pub mod error;
/// The per-plugin live enabled flag `drain_registrations` installs around
/// every capability a static plugin registers.
mod plugin_gate;
/// Bridges `senken_series::Bar` to the WIT wire shapes
/// `senken-plugin-host` speaks, and the catalog of indicators loaded from
/// uploaded `.wasm` components.
pub mod plugin_host;
/// Bar-fetching services: [`SeriesData`], `Runtime::series()`.
mod series;
/// Per-account catalogs of indicators an account compiled from their own
/// Rust source: [`user_indicators::UserIndicators`], `Runtime::user_indicators()`.
pub mod user_indicators;

pub use crate::error::RuntimeError;
pub use crate::plugin_host::{
    DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS, DynamicIndicatorError, DynamicIndicatorInfo,
    DynamicIndicatorInstance, DynamicIndicatorStatus, DynamicIndicators, DynamicOnBar,
    DynamicParamSpec, DynamicPlotSpec, DynamicVenueError, DynamicVenues, PluginOrigin,
    reject_if_over_display_cap,
};
pub use crate::series::SeriesData;
pub use crate::user_indicators::UserIndicators;

/// Where data lives when the builder is given no other location.
pub const DEFAULT_DATA_DIR: &str = ".data";

/// Directory name under the data directory where a `.wasm` indicator
/// component dropped in by hand (never through the upload or compile
/// endpoints) is picked up at startup — see
/// [`scan_indicator_plugin_directory`].
const INDICATOR_PLUGINS_DIR: &str = "indicator-plugins";

/// Static plugins seeded as enabled on a completely fresh `plugin_state`
/// table (see `RuntimeBuilder::build`). Deliberately small: most static
/// plugins are venues that start talking to a real exchange over the
/// network the moment they activate, and an installation should not do
/// that to two dozen of them before anyone has asked it to. `simulator`
/// needs no network at all, and `okx` is the one live venue this project
/// treats as safe to poll by default.
const DEFAULT_ENABLED_STATIC_PLUGINS: &[&str] = &["okx", "simulator"];

/// Registers every `.wasm` file directly under
/// `<data_dir>/`[`INDICATOR_PLUGINS_DIR`] as a
/// [`crate::plugin_host::PluginOrigin::DataDirectory`] dynamic indicator, so
/// a plugin placed there by hand is picked up the moment the runtime
/// starts, the same way an operator dropping a widget package under
/// `<data_dir>/widget-plugins/packages/` is picked up the moment anything
/// calls [`senken_plugin::widget_package::WidgetPackageStore::list`].
///
/// One plugin failing to load never aborts startup. `register_with_origin`
/// already turns a load failure into a visible, named catalog entry (see
/// that method's own docs) rather than only returning an error to its
/// immediate caller — this function logs the failure for the operator and
/// moves on to the next file regardless, so **the failed entry, not a
/// crashed server, is the visible result**. The directory itself is created
/// if it does not exist yet, so a fresh install has somewhere to drop a
/// file into without creating the directory by hand first.
fn scan_indicator_plugin_directory(
    dynamic_indicators: &plugin_host::DynamicIndicators,
    data_dir: &Path,
) {
    let dir = data_dir.join(INDICATOR_PLUGINS_DIR);
    if let Err(source) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            %source,
            path = %dir.display(),
            "could not create the indicator-plugins data directory; skipping the startup scan"
        );
        return;
    }
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(source) => {
            tracing::warn!(
                %source,
                path = %dir.display(),
                "could not read the indicator-plugins data directory; skipping the startup scan"
            );
            return;
        }
    };
    // Sorted so a startup scan registers the same plugins in the same order
    // on every run, regardless of what order the filesystem happens to
    // return directory entries in.
    let mut paths: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "wasm"))
        .collect();
    paths.sort();

    for path in paths {
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(source) => {
                tracing::warn!(
                    %source,
                    path = %path.display(),
                    "could not read a data-directory indicator plugin file"
                );
                continue;
            }
        };
        match dynamic_indicators
            .register_with_origin(&bytes, plugin_host::PluginOrigin::DataDirectory)
        {
            Ok(info) => tracing::info!(
                id = %info.id,
                path = %path.display(),
                "loaded a data-directory indicator plugin"
            ),
            Err(source) => tracing::warn!(
                %source,
                path = %path.display(),
                "a data-directory indicator plugin failed to load; it is recorded as a \
                 failed catalog entry rather than aborting startup"
            ),
        }
    }
}

/// Registers every activated static plugin's own embedded
/// [`Plugin::venue_components`] into `dynamic_venues`.
///
/// Only `records` — plugins that actually activated — are considered, so a
/// venue a `plugin_state` row has disabled never has its components loaded
/// either; disabling the plugin already keeps it out of `records` (see
/// [`activate_static_plugins`]). A plugin that has *not* moved any market to
/// a component returns an empty `Vec` here and this loop simply has nothing
/// to do for it, which is every plugin except one mid-migration.
///
/// One component failing to load never aborts startup, and never stops the
/// rest of that same plugin's other components from loading — for the same
/// reason [`load_dynamic_venue_packages`] does not abort on a package
/// failure: a corrupt or incompatible embedded component is a defect in
/// this build, not something an operator caused, but the catalog entry it
/// produces (a visible `FailedToLoad`/`Incompatible` row, not a crashed
/// server) is still the right way to surface it.
fn load_static_venue_components(
    records: &[PluginRecord],
    dynamic_venues: &crate::plugin_host::DynamicVenues,
) -> HashMap<String, String> {
    // A component's own descriptor id is the *market-data source* id — it
    // becomes half of every instrument id, and a saved chart layout is
    // keyed by that, so it cannot be changed to whatever the plugin is
    // called. The plugin a component was compiled into is a different
    // identity, and one the listing needs: without this map the same venue
    // shows up twice, once as the plugin and once as its own component.
    let mut owners = HashMap::new();
    for record in records {
        for wasm in record.plugin().venue_components() {
            match dynamic_venues.register_with_origin_and_base_url(
                wasm,
                crate::plugin_host::PluginOrigin::BuiltIn,
                None,
            ) {
                Ok(info) => {
                    tracing::info!(
                        id = %info.id,
                        plugin = record.manifest.id,
                        "loaded a static plugin's embedded venue component"
                    );
                    owners.insert(info.id, record.manifest.id.clone());
                }
                Err(source) => tracing::warn!(
                    %source,
                    plugin = record.manifest.id,
                    "a static plugin's embedded venue component failed to load"
                ),
            }
        }
    }
    owners
}

/// Registers every currently-active package's `venue` contribution into
/// `dynamic_venues` — the package equivalent of
/// [`load_static_venue_components`] above, for a venue installed as a
/// package rather than embedded in this binary, run once at startup.
///
/// One package failing to load — a corrupt `.wasm`, an incompatible
/// `senken:plugin-api` version, a probe call trapping — never aborts
/// startup: [`plugin_host::DynamicVenues::register_with_origin_and_base_url`]
/// already turns that failure into a visible, named catalog entry rather
/// than only returning an error to its caller, so this function logs the
/// failure for the operator and moves on to the next package regardless.
fn load_dynamic_venue_packages(
    widget_plugins: &WidgetPackageStore,
    dynamic_venues: &crate::plugin_host::DynamicVenues,
) {
    let venues = match widget_plugins.effective_venue_catalog() {
        Ok(venues) => venues,
        Err(source) => {
            tracing::warn!(%source, "could not read the plugin package store's venue catalog");
            return;
        }
    };
    for (package_id, contribution) in venues {
        let wasm = match widget_plugins.resolve_wasm(&package_id, &contribution.entry) {
            Ok(Some(wasm)) => wasm,
            Ok(None) => {
                tracing::warn!(
                    package = package_id,
                    entry = contribution.entry,
                    "a package declares a venue contribution whose entry file is missing"
                );
                continue;
            }
            Err(source) => {
                tracing::warn!(%source, package = package_id, "could not read a venue package's entry file");
                continue;
            }
        };
        match dynamic_venues.register_with_origin_and_base_url(
            &wasm,
            crate::plugin_host::PluginOrigin::DataDirectory,
            contribution.base_url,
        ) {
            Ok(info) => tracing::info!(
                id = %info.id,
                package = package_id,
                "loaded a package's venue contribution"
            ),
            Err(source) => tracing::warn!(
                %source,
                package = package_id,
                "a package's venue contribution failed to load; it is recorded as a failed \
                 catalog entry rather than aborting startup"
            ),
        }
    }
}

/// Registers every currently-active package's `indicator` contribution into
/// `dynamic_indicators` — the package equivalent of
/// [`load_dynamic_venue_packages`], for the extension point a package
/// declares instead of the historical bare-`.wasm`-file-in-a-directory path
/// [`scan_indicator_plugin_directory`] still serves for compatibility.
///
/// One package failing to load never aborts startup, for the same reason
/// [`load_dynamic_venue_packages`] does not: `register_with_origin` already
/// turns that failure into a visible, named catalog entry rather than only
/// returning an error to its caller.
fn load_dynamic_indicator_packages(
    widget_plugins: &WidgetPackageStore,
    dynamic_indicators: &plugin_host::DynamicIndicators,
) {
    let indicators = match widget_plugins.effective_indicator_catalog() {
        Ok(indicators) => indicators,
        Err(source) => {
            tracing::warn!(%source, "could not read the plugin package store's indicator catalog");
            return;
        }
    };
    for (package_id, contribution) in indicators {
        let wasm = match widget_plugins.resolve_wasm(&package_id, &contribution.entry) {
            Ok(Some(wasm)) => wasm,
            Ok(None) => {
                tracing::warn!(
                    package = package_id,
                    entry = contribution.entry,
                    "a package declares an indicator contribution whose entry file is missing"
                );
                continue;
            }
            Err(source) => {
                tracing::warn!(%source, package = package_id, "could not read an indicator package's entry file");
                continue;
            }
        };
        match dynamic_indicators
            .register_with_origin(&wasm, crate::plugin_host::PluginOrigin::DataDirectory)
        {
            Ok(info) => tracing::info!(
                id = %info.id,
                package = package_id,
                "loaded a package's indicator contribution"
            ),
            Err(source) => tracing::warn!(
                %source,
                package = package_id,
                "a package's indicator contribution failed to load; it is recorded as a failed \
                 catalog entry rather than aborting startup"
            ),
        }
    }
}

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

/// Whether one [`PluginListing`] came from a statically-linked `Plugin`
/// compiled into this binary, or a package discovered under
/// `<data_dir>/widget-plugins/packages/`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginKind {
    /// A `Plugin` compiled into this binary.
    Static,
    /// A package discovered on disk.
    Package,
}

/// The state [`Runtime::plugin_catalog`] reports for one plugin. A
/// coarser view than [`PluginRecord`]/[`plugin_host::DynamicVenueStatus`]
/// individually carry — enough for the Plugins page to render a badge and
/// a reason, not the full health/log detail those types still hold.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginListingState {
    /// Contributing normally.
    Active,
    /// Installed but deliberately turned off.
    Disabled,
    /// Failed to load, or auto-disabled by its circuit breaker — either
    /// way, the reason is shown rather than the plugin silently vanishing
    /// from the list.
    Failed(String),
}

/// One row [`Runtime::plugin_catalog`] reports — the same shape for a
/// static venue and a package, so the Plugins page reads one list rather
/// than stitching two together itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginListing {
    /// Stable identifier.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Version string (empty for a dynamic venue with no manifest of its
    /// own to read one from yet).
    pub version: String,
    /// Static or package — see [`PluginKind`].
    pub kind: PluginKind,
    /// What this plugin declares it contributes.
    pub contributes: Vec<ContributionKind>,
    /// Its current state — see [`PluginListingState`].
    pub state: PluginListingState,
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

        let mut bar_sources: Vec<Arc<dyn BarSource>> = Vec::new();
        let mut book_sources: Vec<Arc<dyn BookSource>> = Vec::new();
        let mut feed_sources: Vec<Arc<dyn FeedSource>> = Vec::new();
        let mut trade = TradeEngine::new();
        // One context for the whole run, so resources it caches (the shared
        // HTTP client) are shared by every plugin.
        let mut context = ActivationContext::new();

        let (records, static_catalog, static_plugin_gates, stored_static_plugins) =
            activate_static_plugins(
                self.plugins,
                self.identity.as_deref(),
                &mut context,
                Registries {
                    marketdata: &marketdata,
                    bar_sources: &mut bar_sources,
                    book_sources: &mut book_sources,
                    feed_sources: &mut feed_sources,
                    trade: Some(&mut trade),
                },
            )?;

        let widget_plugins = WidgetPackageStore::open(storage.data_dir())
            .map_err(|source| RuntimeError::WidgetPluginStoreInit { source })?;
        // A fresh server has nothing installed on either plugin surface —
        // this is the one widget UI package that ships regardless, so the
        // dashboard's "add widget" picker and Settings' widget plugin
        // manager are never simply empty. Already being present (every
        // start after the first) is a no-op — see this method's own docs.
        widget_plugins
            .ensure_builtin_installed()
            .map_err(|source| RuntimeError::WidgetPluginStoreInit { source })?;

        let dynamic_venues = crate::plugin_host::DynamicVenues::new()
            .map_err(|source| RuntimeError::DynamicVenueHostInit { source })?;
        let static_venue_components = load_static_venue_components(&records, &dynamic_venues);
        load_dynamic_venue_packages(&widget_plugins, &dynamic_venues);
        for source in dynamic_venues.marketdata_sources() {
            // A venue package registers itself the same way a compiled-in
            // plugin does: registration is the capability declaration.
            // Duplicate ids are rejected exactly the same way, too — a
            // package cannot silently shadow a compiled-in venue's id.
            marketdata.register_source(source).map_err(|source| {
                RuntimeError::SourceRegistration {
                    plugin: "dynamic-venue-package".to_owned(),
                    source,
                }
            })?;
        }
        bar_sources.extend(dynamic_venues.bar_sources());

        let marketdata = Arc::new(marketdata);

        // Rooted at the same data directory as everything else;
        // `senken-store`'s `sources/{id}/instruments/{KEY}/bars/...` layout
        // lives alongside `senken-marketdata`'s own
        // `sources/{id}/instruments.json` under one `.data` tree.
        let series_store = Store::new(storage.data_dir());
        series_store
            .init()
            .map_err(|source| RuntimeError::SeriesStoreInit { source })?;
        let series = SeriesData::build(&series_store, &marketdata, bar_sources);

        let dynamic_indicators = crate::plugin_host::DynamicIndicators::new()
            .map_err(|source| RuntimeError::DynamicIndicatorHostInit { source })?;
        load_dynamic_indicator_packages(&widget_plugins, &dynamic_indicators);
        scan_indicator_plugin_directory(&dynamic_indicators, storage.data_dir());

        // A separate `PluginHost` (its own `wasmtime::Engine`) from
        // `dynamic_indicators`'s: user-compiled indicators are isolated
        // per account (see `user_indicators`'s own module docs for why),
        // not a fourth origin sharing the admin-managed catalog above.
        let user_indicators_host =
            senken_plugin_host::PluginHost::new(senken_plugin_host::PluginLimits::default())
                .map_err(crate::plugin_host::DynamicIndicatorError::from)
                .map_err(|source| RuntimeError::DynamicIndicatorHostInit { source })?;
        let user_indicators = crate::user_indicators::UserIndicators::new(user_indicators_host);
        reload_compiled_user_indicators(&user_indicators, self.identity.as_deref());

        Ok(Runtime {
            storage,
            plugins: records,
            static_venue_components: RwLock::new(static_venue_components),
            static_catalog,
            static_plugin_gates: RwLock::new(static_plugin_gates),
            marketdata,
            series,
            series_store,
            book_sources: RwLock::new(
                book_sources
                    .into_iter()
                    .map(|source| (source.source_id().to_owned(), source))
                    .collect(),
            ),
            feed_sources: RwLock::new(feed_sources),
            dynamic_indicators,
            user_indicators,
            widget_plugins,
            dynamic_venues,
            trade: Arc::new(trade),
            stored_static_plugins: Mutex::new(stored_static_plugins),
            late_activated_plugins: Mutex::new(Vec::new()),
            identity: self.identity,
        })
    }
}

/// Loads every indicator that has ever compiled successfully, across every
/// account, into `user_indicators`'s per-owner catalogs — the equivalent,
/// for a compiled-Rust-by-account indicator, of
/// [`scan_indicator_plugin_directory`] for a file dropped in by hand.
///
/// `identity` is `None` when this runtime was built with no
/// [`IdentityStore`] at all (`RuntimeBuilder::identity_store` was never
/// called) — there are no accounts to have compiled anything in that case,
/// so this is a no-op rather than an error. One indicator failing to
/// reload never aborts startup, the same way one bad file in the
/// indicator-plugins directory does not.
fn reload_compiled_user_indicators(
    user_indicators: &crate::user_indicators::UserIndicators,
    identity: Option<&IdentityStore>,
) {
    let Some(identity) = identity else {
        return;
    };
    let store = senken_indicator_registry::UserIndicatorStore::new(identity);
    let rows = match store.all_compiled_for_startup() {
        Ok(rows) => rows,
        Err(source) => {
            tracing::warn!(%source, "could not list compiled user indicators at startup");
            return;
        }
    };
    for row in rows {
        let name = format!("my/{}", row.slug);
        if let Err(source) = user_indicators.load(row.owner_id, &name, &row.wasm) {
            tracing::warn!(
                %source,
                owner = %row.owner_id,
                name,
                "a user's compiled indicator failed to reload at startup"
            );
        }
    }
}

/// The registries a plugin's activation contributes into, passed as one so
/// [`activate`]'s signature does not grow a parameter per capability — a
/// count that grows every time the plugin contract does.
///
/// `marketdata` takes `&MarketData`, not `&mut`, because
/// `senken_marketdata::MarketData::register_source` itself takes `&self` —
/// see that method's own docs for why. `trade` is `Option` rather than a
/// bare `&mut TradeEngine`: at startup there is a unique `&mut TradeEngine`
/// to hand over (`Some`), but a plugin activated later, once the server is
/// already running, only ever has `Runtime`'s `Arc<TradeEngine>` to work
/// with — there is no `&mut` left to give, so late activation passes
/// `None` and [`drain_registrations`] refuses outright if the plugin then
/// tries to register a trade adapter anyway (see that function's own
/// docs, and `Runtime::activate_stored_plugin`'s, for why that refusal is
/// itself the point rather than a bug).
struct Registries<'a> {
    marketdata: &'a MarketData,
    bar_sources: &'a mut Vec<Arc<dyn BarSource>>,
    book_sources: &'a mut Vec<Arc<dyn BookSource>>,
    feed_sources: &'a mut Vec<Arc<dyn FeedSource>>,
    trade: Option<&'a mut TradeEngine>,
}

/// Decides every static plugin's enabled state against `plugin_state`
/// (seeding the default set first, on a completely empty table — see
/// [`DEFAULT_ENABLED_STATIC_PLUGINS`]), then activates only the ones that
/// are enabled. A plugin skipped this way is never given a chance to
/// register anything — it is a catalog entry, not a running plugin — and a
/// duplicate id is rejected the same way whether or not either copy would
/// have activated.
///
/// Pulled out of [`RuntimeBuilder::build`] to keep that function within
/// this workspace's line-count lint rather than to be reused — it has
/// exactly one caller.
/// What activating every static plugin produced: the activated plugins
/// themselves, the catalog row for every one of them (activated or not),
/// the live enabled-flag gate for each one that did activate — see
/// `Runtime::static_plugin_gates`'s own field docs for why only those have
/// one — and every plugin that built successfully but was left disabled,
/// kept so it can be turned on later without a restart (see
/// `Runtime::activate_stored_plugin`).
type StaticActivationOutcome = (
    Vec<PluginRecord>,
    Vec<PluginListing>,
    HashMap<String, plugin_gate::PluginGate>,
    HashMap<String, Box<dyn Plugin>>,
);

fn activate_static_plugins(
    plugins: Vec<Box<dyn Plugin>>,
    identity: Option<&IdentityStore>,
    context: &mut ActivationContext,
    registries: Registries<'_>,
) -> Result<StaticActivationOutcome, RuntimeError> {
    let Registries {
        marketdata,
        bar_sources,
        book_sources,
        feed_sources,
        mut trade,
    } = registries;

    let mut records: Vec<PluginRecord> = Vec::with_capacity(plugins.len());
    let mut static_catalog: Vec<PluginListing> = Vec::with_capacity(plugins.len());
    let mut gates: HashMap<String, plugin_gate::PluginGate> = HashMap::with_capacity(plugins.len());
    let mut stored: HashMap<String, Box<dyn Plugin>> = HashMap::new();
    let mut seen = HashSet::with_capacity(plugins.len());

    // A completely empty `plugin_state` means nobody has ever decided
    // anything — including this being the very first start ever. Seed it
    // so that case reads as "only these are on" rather than "every
    // compiled-in venue is on", which is what an untouched table would
    // otherwise mean below. A later start, once any row exists (from this
    // seed or an admin's own toggle), leaves the table alone.
    if let Some(identity) = identity {
        identity
            .seed_default_plugin_state(DEFAULT_ENABLED_STATIC_PLUGINS)
            .map_err(|source| RuntimeError::PluginStateInit { source })?;
    }

    for plugin in plugins {
        let manifest = plugin.manifest();
        if !seen.insert(manifest.id.clone()) {
            return Err(RuntimeError::DuplicatePlugin(manifest.id.clone()));
        }

        // No identity store at all means this runtime has no way to read
        // anyone's decision — every headless embedding and most of this
        // crate's own tests build a runtime that way, and they rely on
        // every registered plugin activating unconditionally, exactly as
        // it always has. Once an identity store backs the runtime,
        // `plugin_state` becomes the source of truth, and a plugin with no
        // row of its own (not seeded, never toggled) now defaults to *not*
        // activating — see the seed above for the one carve-out.
        let enabled = match identity {
            Some(identity) => identity
                .plugin_enabled(&manifest.id)
                .unwrap_or_else(|source| {
                    tracing::warn!(
                        %source,
                        plugin = manifest.id,
                        "could not read a static plugin's stored enabled flag; treating it as disabled"
                    );
                    None
                })
                .unwrap_or(false),
            None => true,
        };

        if !enabled {
            tracing::info!(
                plugin = manifest.id,
                "static plugin is disabled; not activating"
            );
            static_catalog.push(PluginListing {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                version: manifest.version.clone(),
                kind: PluginKind::Static,
                contributes: manifest.contributes.clone(),
                state: PluginListingState::Disabled,
            });
            // Kept, not dropped: this is exactly the plugin
            // `Runtime::activate_stored_plugin` needs later to turn this
            // one on without a restart.
            stored.insert(manifest.id.clone(), plugin);
            continue;
        }

        // Fresh and enabled — installed around every capability this
        // plugin is about to register (see `drain_registrations`), never
        // something the plugin itself has to remember to wire up.
        let gate = plugin_gate::new_gate();
        if let Err(error) = activate(
            &*plugin,
            &manifest,
            context,
            Registries {
                marketdata,
                bar_sources: &mut *bar_sources,
                book_sources: &mut *book_sources,
                feed_sources: &mut *feed_sources,
                trade: trade.as_deref_mut(),
            },
            identity,
            &gate,
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
        static_catalog.push(PluginListing {
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.clone(),
            kind: PluginKind::Static,
            contributes: manifest.contributes.clone(),
            state: PluginListingState::Active,
        });
        gates.insert(manifest.id.clone(), gate);
        records.push(PluginRecord {
            plugin,
            manifest,
            activated_at: SystemTime::now(),
        });
    }

    Ok((records, static_catalog, gates, stored))
}

fn activate(
    plugin: &dyn Plugin,
    manifest: &PluginManifest,
    context: &mut ActivationContext,
    registries: Registries<'_>,
    identity: Option<&IdentityStore>,
    gate: &plugin_gate::PluginGate,
) -> Result<(), RuntimeError> {
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
        drop(context.take_book_sources());
        drop(context.take_feed_sources());
        drop(context.take_trade_adapters());
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

    drain_registrations(manifest, context, registries, gate)
}

/// Moves what one plugin registered out of the shared context and into the
/// runtime's own registries, leaving the context ready for the next plugin.
///
/// Every `MarketDataSource`/`BarSource`/`BookSource`/`FeedSource` is
/// wrapped with `gate` before it reaches any other crate — see
/// `plugin_gate`'s own module docs for why installing it here, rather than
/// asking a plugin to check its own flag, is what makes the mistake
/// unrepresentable.
///
/// A registered `TradeAdapter` is the one exception, and deliberately so:
/// an adapter already holds a broker's credentials and may be managing
/// open positions, and deciding that trading through it stops the instant
/// an admin flips a switch is not a call this runtime makes on its own.
/// Disabling such a plugin still persists the admin's choice (see
/// `Runtime::set_static_plugin_enabled`) and still mutes whatever venue
/// capability the same plugin also registered, but its trade adapter stays
/// reachable through `TradeEngine` until the process restarts —
/// `Runtime::plugin_catalog` reports that honestly rather than claiming
/// the plugin is fully off.
fn drain_registrations(
    manifest: &PluginManifest,
    context: &mut ActivationContext,
    registries: Registries<'_>,
    gate: &plugin_gate::PluginGate,
) -> Result<(), RuntimeError> {
    let Registries {
        marketdata,
        bar_sources,
        book_sources,
        feed_sources,
        trade,
    } = registries;

    for source in context.take_marketdata_sources() {
        tracing::info!(
            plugin = manifest.id,
            source = source.id(),
            "registering marketdata source"
        );
        let source = Arc::new(plugin_gate::GatedMarketDataSource::new(
            source,
            Arc::clone(gate),
        ));
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
        bar_sources.push(Arc::new(plugin_gate::GatedBarSource::new(
            source,
            Arc::clone(gate),
        )));
    }
    for source in context.take_book_sources() {
        tracing::info!(
            plugin = manifest.id,
            source = source.source_id(),
            "registering book source"
        );
        book_sources.push(Arc::new(plugin_gate::GatedBookSource::new(
            source,
            Arc::clone(gate),
        )));
    }
    for source in context.take_feed_sources() {
        tracing::info!(
            plugin = manifest.id,
            sources = ?source.source_ids(),
            "registering live feed"
        );
        feed_sources.push(Arc::new(plugin_gate::GatedFeedSource::new(
            source,
            Arc::clone(gate),
        )));
    }

    let trade_adapters = context.take_trade_adapters();
    if !trade_adapters.is_empty() {
        // `trade` is `None` only when this activation is running live,
        // after startup (see `Registries`'s own docs) — `TradeEngine` is
        // shared behind an `Arc` by then, with no unique `&mut` left to
        // register into, and this project has separately decided a switch
        // must never grant one anyway (see this crate's module docs on the
        // trade-adapter scope decision). A manifest that declares
        // `ContributionKind::TradeAdapter` is refused before `activate`
        // even runs (`Runtime::activate_stored_plugin`), so reaching this
        // with adapters in hand and nowhere to put them means a plugin
        // registered one without declaring it — a plugin bug worth
        // surfacing loudly rather than silently dropping the adapter.
        let Some(trade) = trade else {
            return Err(RuntimeError::TradeAdapterRegistration {
                plugin: manifest.id.clone(),
                source: senken_trade::TradeError::unsupported(
                    manifest.id.clone(),
                    "registering a trade adapter without restarting Senken",
                ),
            });
        };
        for adapter in trade_adapters {
            tracing::info!(
                plugin = manifest.id,
                adapter = adapter.id(),
                "registering trade adapter"
            );
            trade
                .register(adapter)
                .map_err(|source| RuntimeError::TradeAdapterRegistration {
                    plugin: manifest.id.clone(),
                    source,
                })?;
        }
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
    /// Which static plugin each embedded venue component belongs to, keyed
    /// by the component's own descriptor id. The two identities differ on
    /// purpose: the descriptor id is the market-data source id a saved
    /// layout stores, while the plugin id is what a reader switches on and
    /// off. Listing both as separate rows would show one venue twice.
    static_venue_components: RwLock<HashMap<String, String>>,
    /// Every static plugin `RuntimeBuilder` was given, active or not —
    /// computed once at build time. Unlike `plugins`, this includes the
    /// ones that never activated at all, which is exactly what makes a
    /// disabled built-in venue still show up on the Plugins page.
    ///
    /// Whether a plugin *activated* is fixed here forever — that only ever
    /// happens once, at startup. Whether it is *currently contributing*
    /// is a different question for a plugin that did activate, and
    /// [`Self::plugin_catalog`] answers it by reading
    /// [`Self::static_plugin_gates`] instead of trusting this field's own
    /// `state` blindly; see that method's own docs.
    static_catalog: Vec<PluginListing>,
    /// One live on/off flag per static plugin that activated at startup, or
    /// afterward through `Self::activate_stored_plugin` — absent entirely
    /// for one that never activated at all (see
    /// [`Self::set_static_plugin_enabled`], the only way this map gains an
    /// entry after `build()`). Shared with every capability
    /// `drain_registrations` wrapped for that plugin (see `plugin_gate`'s
    /// own module docs), so flipping the flag here is observed immediately
    /// by every `MarketDataSource`/`BarSource`/`BookSource`/`FeedSource`
    /// already registered — no second registration step, the same
    /// property [`plugin_host::DynamicVenues`] already has for a package.
    ///
    /// Behind a `RwLock` rather than fixed at build time: a plugin that
    /// activates after startup needs a place to insert its own gate into
    /// an already-running `Runtime`, and every read here (`plugin_catalog`,
    /// `set_static_plugin_enabled`'s own lookup) is a short, uncontended
    /// lock to clone an `Arc`, never held across a capability call.
    static_plugin_gates: RwLock<HashMap<String, plugin_gate::PluginGate>>,
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
    ///
    /// Behind a `RwLock` for the same reason [`Self::static_plugin_gates`]
    /// is: `Self::activate_stored_plugin` inserts into this after
    /// startup, and every reader (`book_source`, `has_book_source`) only
    /// ever holds the lock long enough to clone an `Arc` or test a key.
    book_sources: RwLock<HashMap<String, Arc<dyn BookSource>>>,
    /// Every live feed the active plugins registered. Not yet connected:
    /// a [`FeedSource`] is the means to build a protocol once a catalog
    /// exists, and whoever runs live data decides when that happens — see
    /// `Self::activate_stored_plugin`'s own docs for why a feed
    /// registered by a plugin activated after server startup still has no
    /// live [`senken_subscription::SubscriptionPool`] this run.
    ///
    /// Behind a `RwLock` for the same reason [`Self::book_sources`] is.
    feed_sources: RwLock<Vec<Arc<dyn FeedSource>>>,
    /// Indicators loaded from an uploaded `.wasm` component, alongside the
    /// ten built-ins `senken-indicators` ships with.
    dynamic_indicators: crate::plugin_host::DynamicIndicators,
    /// Indicators compiled from an account's own Rust source, one catalog
    /// per account — never merged into `dynamic_indicators` above, which
    /// is shared by everyone. See `user_indicators`'s own module docs.
    user_indicators: crate::user_indicators::UserIndicators,
    /// Dynamic widget UI packages, rooted at the same data directory as
    /// everything else. Unlike `dynamic_indicators`, this store needs no
    /// startup scan of its own here: it re-reads its `packages/` directory
    /// from disk on every `list`/`refresh` call rather than caching an
    /// in-memory catalog (see that type's own docs), so a package dropped
    /// in by hand is already visible the moment anything asks — opening it
    /// here only has to happen once, at startup, so every caller shares one
    /// instance instead of each opening (and re-preparing the directory
    /// for) its own.
    widget_plugins: WidgetPackageStore,
    /// Venues loaded from a package's `venue` contribution, alongside the
    /// static `Plugin` venues activated above. Disabling one here empties
    /// its instrument catalog and rejects new bar fetches without removing
    /// it from this registry — see [`plugin_host::DynamicVenues`]'s own
    /// docs for why, and why its catalog is merged into
    /// [`marketdata`](Self::marketdata) and [`series`](Self::series) at
    /// build time rather than queried separately by every caller.
    dynamic_venues: crate::plugin_host::DynamicVenues,
    /// Every trade adapter the activated plugins registered. An `Arc` so
    /// the HTTP layer can hold it for the process's lifetime, the same way
    /// it holds [`marketdata`](Self::marketdata).
    trade: Arc<TradeEngine>,
    /// Static plugins that built successfully at startup but were left
    /// disabled, keyed by manifest id — exactly what
    /// `Self::activate_stored_plugin` drains from. A plugin id present
    /// here has never activated this run; one absent has either activated
    /// already (at startup or later) or never existed at all — see that
    /// method's own docs for why draining an entry out of this map, not
    /// merely checking it, is what makes a second activation of the same
    /// plugin structurally impossible.
    stored_static_plugins: Mutex<HashMap<String, Box<dyn Plugin>>>,
    /// Every plugin activated after startup through
    /// `Self::activate_stored_plugin` — kept only so [`Self::shutdown`]/
    /// [`Drop`] deactivate it too. [`Self::plugins`] itself still reports
    /// only what activated at startup; see that method's own docs.
    late_activated_plugins: Mutex<Vec<PluginRecord>>,
    /// The identity store [`RuntimeBuilder::identity_store`] was given, if
    /// any — kept here (not just consumed at `build()` time) so
    /// `Self::activate_stored_plugin` can reconcile a late-activated
    /// plugin's declared permissions exactly the way [`activate`] already
    /// does at startup, rather than skipping that step for a plugin
    /// activated after the server is already running.
    identity: Option<Arc<IdentityStore>>,
}

/// The user-facing state a dynamically loaded component's own state maps to.
///
/// Venues and indicators load through the same host and carry the same
/// state type, so they read it the same way — the version mismatch is
/// spelled out here rather than at each call site because a reader of the
/// Plugins page has no other way to learn which build supports what.
fn listing_state(state: crate::plugin_host::DynamicIndicatorState) -> PluginListingState {
    match state {
        crate::plugin_host::DynamicIndicatorState::Active => PluginListingState::Active,
        crate::plugin_host::DynamicIndicatorState::Disabled => PluginListingState::Disabled,
        crate::plugin_host::DynamicIndicatorState::AutoDisabled { reason }
        | crate::plugin_host::DynamicIndicatorState::FailedToLoad { reason } => {
            PluginListingState::Failed(reason)
        }
        crate::plugin_host::DynamicIndicatorState::Incompatible {
            found_version,
            supported_version,
        } => PluginListingState::Failed(format!(
            "requires plugin API {found_version}, this build supports {supported_version}"
        )),
    }
}

/// Overrides a live-gated static plugin's `state` in place — `out`'s own
/// copy (cloned from [`Runtime::static_catalog`]) is fixed at boot time and
/// never reflects a toggle made after startup through
/// [`Runtime::set_static_plugin_enabled`].
///
/// A plugin that also registered a [`senken_trade::TradeAdapter`] is left
/// untouched here: that capability cannot be turned off without a restart
/// (see `drain_registrations`'s own docs), so its listing must keep
/// reporting active for as long as it does, never claim "disabled" while
/// it still trades.
fn apply_live_plugin_gates(
    out: &mut [PluginListing],
    gates: &HashMap<String, plugin_gate::PluginGate>,
) {
    for listing in out {
        if listing
            .contributes
            .contains(&ContributionKind::TradeAdapter)
        {
            continue;
        }
        if let Some(gate) = gates.get(&listing.id) {
            listing.state = if gate.load(Ordering::Relaxed) {
                PluginListingState::Active
            } else {
                PluginListingState::Disabled
            };
        }
    }
}

/// Why a static plugin's live enabled flag could not be changed by
/// [`Runtime::set_static_plugin_enabled`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StaticPluginToggleError {
    /// No static plugin is registered under this id at all.
    #[error("no static plugin is registered as `{0}`")]
    UnknownPlugin(String),
    /// `id` names a real static plugin, but it never activated this run —
    /// its `plugin_state` row (or the fresh-install default) was already
    /// off, so it registered nothing here for a live flag to gate. Turning
    /// it on needs a restart; turning off a plugin that is already off is
    /// answered as a no-op instead, never this error.
    #[error("`{0}` did not activate at startup and cannot be enabled without a restart")]
    NeverActivated(String),

    /// `id` names a static plugin that was stored, disabled, since startup,
    /// and this call tried to activate it live — but the attempt itself
    /// failed (`Plugin::activate` returned an error, or a capability it
    /// registered was rejected, most likely a duplicate id). The plugin is
    /// gone from the stored set either way (see
    /// `Runtime::activate_stored_plugin`'s own docs); enabling it again
    /// needs a restart.
    #[error("`{plugin}` failed to activate: {reason}")]
    ActivationFailed {
        /// The plugin's manifest id.
        plugin: String,
        /// What went wrong.
        reason: String,
    },
}

impl Runtime {
    /// Loads a plugin's embedded venue components into `dynamic_venues` as
    /// it is switched on.
    ///
    /// A venue that moved its markets into a component registers nothing for
    /// them through `ActivationContext` — the component is the only thing
    /// serving them — so without this the plugin would switch on while its
    /// charts stayed empty, which is worse than an honest "restart needed".
    fn load_venue_components_of(&self, id: &str, plugin: &dyn Plugin) {
        let mut loaded: Vec<String> = Vec::new();
        for wasm in plugin.venue_components() {
            match self.dynamic_venues.register_with_origin_and_base_url(
                wasm,
                crate::plugin_host::PluginOrigin::BuiltIn,
                None,
            ) {
                Ok(info) => {
                    self.static_venue_components
                        .write()
                        .unwrap_or_else(PoisonError::into_inner)
                        .insert(info.id.clone(), id.to_owned());
                    loaded.push(info.id);
                }
                Err(source) => tracing::warn!(
                    %source,
                    plugin = id,
                    "a venue component failed to load while enabling this plugin"
                ),
            }
        }
        if loaded.is_empty() {
            return;
        }

        // Holding a component in `dynamic_venues` is not the same as
        // serving from it: startup pulls each one's instruments and bars
        // into the registries every reader actually goes through, and a
        // component loaded later has to make that same trip, or the venue
        // switches on with a catalog nothing can see and a chart that
        // cannot draw it.
        for source in self.dynamic_venues.marketdata_sources() {
            if !loaded.iter().any(|new| new == source.id()) {
                continue;
            }
            if let Err(source) = self.marketdata.register_source(source) {
                tracing::warn!(%source, plugin = id, "a venue component's catalog could not be registered");
            }
        }
        for source in self.dynamic_venues.bar_sources() {
            if loaded.iter().any(|new| new == source.source_id()) {
                self.series
                    .insert(&self.series_store, &self.marketdata, source);
            }
        }
    }

    /// The order-book source registered for `source_id`, if any.
    ///
    /// `None` is a real answer — the venue serves no depth in this build —
    /// not a lookup failure, and a caller reporting capabilities reads it
    /// as such. Returns an owned, cheaply-cloned `Arc` rather than a
    /// reference: a reference into the lock behind this could not outlive
    /// the call, now that a plugin can register depth after startup too
    /// (see `Self::activate_stored_plugin`).
    #[must_use]
    pub fn book_source(&self, source_id: &str) -> Option<Arc<dyn BookSource>> {
        self.book_sources
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(source_id)
            .cloned()
    }

    /// Whether any plugin registered depth for `source_id`.
    #[must_use]
    pub fn has_book_source(&self, source_id: &str) -> bool {
        self.book_sources
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(source_id)
    }

    /// Every live feed the active plugins registered, at startup or later
    /// (see `Self::activate_stored_plugin`). Returns an owned snapshot —
    /// cheap, since cloning just bumps each entry's `Arc` refcount — rather
    /// than a slice reference, since a reference into the lock behind this
    /// could not outlive the call.
    #[must_use]
    pub fn feed_sources(&self) -> Vec<Arc<dyn FeedSource>> {
        self.feed_sources
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
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

    /// Every plugin activated at startup, in activation order. A plugin
    /// turned on afterward through `Self::activate_stored_plugin` is not
    /// reflected here — it is still deactivated at shutdown (see
    /// `Self::all_activated_plugins`) — since nothing today reads this
    /// accessor for anything but the boot-time set it has always reported.
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

    /// Indicators loaded from an uploaded `.wasm` component, alongside the
    /// ten built-ins `senken-indicators` ships with — what
    /// `indicator_handlers` merges into `GET /api/indicators`'s catalogue
    /// and dispatches `POST /api/indicators/compute` against when a
    /// request names something other than a built-in.
    #[must_use]
    pub fn dynamic_indicators(&self) -> &crate::plugin_host::DynamicIndicators {
        &self.dynamic_indicators
    }

    /// Indicators compiled from an account's own Rust source — what
    /// `user_indicator_handlers` loads into after a successful compile and
    /// dispatches `POST /api/my/indicators/{id}/compile`'s
    /// `compute`-equivalent against, and what `indicator_handlers` merges
    /// into `GET /api/indicators` for that account alone (never for
    /// anyone else — see [`Self::dynamic_indicators`] for the catalog
    /// that *is* shared).
    #[must_use]
    pub fn user_indicators(&self) -> &crate::user_indicators::UserIndicators {
        &self.user_indicators
    }

    /// Dynamic widget UI packages: install, discovery, enable/disable, and
    /// the effective widget catalog every currently active package
    /// contributes — what `widget_plugin_handlers` exposes over HTTP.
    #[must_use]
    pub fn widget_plugins(&self) -> &WidgetPackageStore {
        &self.widget_plugins
    }

    /// Venues loaded from an installed package's `venue` contribution.
    #[must_use]
    pub fn dynamic_venues(&self) -> &crate::plugin_host::DynamicVenues {
        &self.dynamic_venues
    }

    /// Flips a **static** plugin's live enabled flag — the compiled-in
    /// counterpart to [`plugin_host::DynamicVenues::set_enabled`], applying
    /// immediately to every `MarketDataSource`/`BarSource`/`BookSource`/
    /// `FeedSource` that plugin registered (see `plugin_gate`'s own module
    /// docs for what "disabled" means for each). A registered
    /// [`senken_trade::TradeAdapter`] is never touched by this call — see
    /// `drain_registrations`'s own docs for why.
    ///
    /// Also flips every dynamic-venue component this plugin owns, found by
    /// the runtime's own component-to-owner map: a static plugin that ported
    /// its instrument/bar catalog to a component (`Plugin::venue_components`)
    /// registers nothing here at all for that market — the component is the
    /// only thing serving it — so without this, disabling the plugin would
    /// leave that component's catalog completely untouched. A component
    /// that failed to load in the first place is not toggleable either way;
    /// that is logged and skipped rather than failing this whole call.
    ///
    /// This does not itself persist the admin's choice — pair it with
    /// `senken_identity::IdentityStore::set_plugin_enabled` the same way
    /// `senken-api`'s `set_plugin_enabled` handler does, so the choice
    /// survives a restart too.
    ///
    /// A plugin that has never activated this run — no gate exists for
    /// `id` yet — is turned on live too, through
    /// `Self::activate_stored_plugin`, **unless** it declares
    /// [`ContributionKind::TradeAdapter`]: that one case still needs a
    /// restart, on purpose (see that method's own docs). Disabling a
    /// plugin that never activated is answered as the no-op it already is.
    ///
    /// # Errors
    /// [`StaticPluginToggleError::UnknownPlugin`] if `id` names no static
    /// plugin at all; [`StaticPluginToggleError::NeverActivated`] if `id`
    /// is a real static plugin that declares a trade adapter, so activating
    /// it live is refused on purpose; or
    /// [`StaticPluginToggleError::ActivationFailed`] if a never-activated
    /// plugin's live activation itself failed.
    pub fn set_static_plugin_enabled(
        &self,
        id: &str,
        enabled: bool,
    ) -> Result<(), StaticPluginToggleError> {
        let gate = self
            .static_plugin_gates
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned();
        let Some(gate) = gate else {
            if self.static_catalog.iter().any(|listing| listing.id == id) {
                return if enabled {
                    self.activate_stored_plugin(id)
                } else {
                    Ok(())
                };
            }
            return Err(StaticPluginToggleError::UnknownPlugin(id.to_owned()));
        };
        gate.store(enabled, Ordering::Relaxed);
        for (component_id, owner) in self
            .static_venue_components
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
        {
            if owner != id {
                continue;
            }
            if let Err(source) = self.dynamic_venues.set_enabled(component_id, enabled) {
                tracing::warn!(
                    %source,
                    plugin = id,
                    component = component_id,
                    "could not apply a static plugin's toggle to one of its own venue components"
                );
            }
        }
        Ok(())
    }

    /// Turns on a static plugin that built successfully at startup but was
    /// left disabled — the live-activation counterpart to `activate` for a
    /// plugin that never got that call at boot. Called only from
    /// [`Self::set_static_plugin_enabled`], never directly by an API
    /// handler, so the trade-adapter refusal below is never bypassable
    /// through a second entry point.
    ///
    /// The plugin's own manifest is peeked, not removed, before anything
    /// else: one that declares [`ContributionKind::TradeAdapter`] is
    /// refused outright and left exactly where it was, because
    /// `senken_trade::TradeEngine` is shared behind an `Arc` once the
    /// server is running — there is no unique `&mut TradeEngine` here to
    /// register into even if this project wanted to grant one, and it has
    /// separately decided a switch must never cut off or grant trading
    /// live anyway (see this crate's module docs). Such a plugin still
    /// needs a restart, exactly as it always has.
    ///
    /// Every other plugin is drained out of [`Self::stored_static_plugins`]
    /// — removed, not merely checked — before activation is even attempted.
    /// That ordering is what makes a duplicate activation of the same
    /// plugin structurally impossible rather than a race this function
    /// happens to usually win: a second call for the same id finds nothing
    /// left in the map, whether or not the first call's activation went on
    /// to succeed. A plugin whose activation then fails is gone for the
    /// rest of this run — the same all-or-nothing contract a boot-time
    /// failure already has in [`activate_static_plugins`], narrowed to
    /// just this one plugin instead of aborting the whole startup.
    ///
    /// Every `MarketDataSource`/`BarSource`/`BookSource`/`FeedSource` this
    /// plugin registers is wrapped by the same [`plugin_gate`] boot-time
    /// activation installs, flowing through the very same
    /// [`drain_registrations`] function — never a second registration path
    /// a future capability could slip past ungated. A registered
    /// `BarSource` is handed to [`SeriesData::insert`] and a `BookSource`
    /// into [`Self::book_sources`] so both become immediately queryable; a
    /// registered `FeedSource` is recorded in [`Self::feed_sources`] too,
    /// but — see that field's own docs — gets no live
    /// [`senken_subscription::SubscriptionPool`] this run, since
    /// `senken_api`'s pools are built once at server startup from a
    /// snapshot of this list. `senken-api`'s plugin handler reports that
    /// gap honestly through the same `needs_restart`/`restart_reason`
    /// fields it already uses for a trade adapter that cannot be turned
    /// off live.
    fn activate_stored_plugin(&self, id: &str) -> Result<(), StaticPluginToggleError> {
        let declares_trade_adapter = {
            let stored = self
                .stored_static_plugins
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(plugin) = stored.get(id) else {
                return Err(StaticPluginToggleError::NeverActivated(id.to_owned()));
            };
            plugin
                .manifest()
                .contributes
                .contains(&ContributionKind::TradeAdapter)
        };
        if declares_trade_adapter {
            return Err(StaticPluginToggleError::NeverActivated(id.to_owned()));
        }

        let plugin = {
            let mut stored = self
                .stored_static_plugins
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let Some(plugin) = stored.remove(id) else {
                // Raced with another activation attempt for the same id
                // between the peek above and this removal.
                return Err(StaticPluginToggleError::NeverActivated(id.to_owned()));
            };
            plugin
        };

        let manifest = plugin.manifest();
        let gate = plugin_gate::new_gate();
        let mut context = ActivationContext::new();
        let mut bar_sources: Vec<Arc<dyn BarSource>> = Vec::new();
        let mut book_sources: Vec<Arc<dyn BookSource>> = Vec::new();
        let mut feed_sources: Vec<Arc<dyn FeedSource>> = Vec::new();

        let outcome = activate(
            plugin.as_ref(),
            &manifest,
            &mut context,
            Registries {
                marketdata: &self.marketdata,
                bar_sources: &mut bar_sources,
                book_sources: &mut book_sources,
                feed_sources: &mut feed_sources,
                trade: None,
            },
            self.identity.as_deref(),
            &gate,
        );

        if let Err(source) = outcome {
            tracing::warn!(%source, plugin = id, "a stored plugin failed to activate live");
            return Err(StaticPluginToggleError::ActivationFailed {
                plugin: id.to_owned(),
                reason: source.to_string(),
            });
        }

        for source in bar_sources {
            self.series
                .insert(&self.series_store, &self.marketdata, source);
        }
        if !book_sources.is_empty() {
            let mut sources = self
                .book_sources
                .write()
                .unwrap_or_else(PoisonError::into_inner);
            for source in book_sources {
                sources.insert(source.source_id().to_owned(), source);
            }
        }
        if !feed_sources.is_empty() {
            self.feed_sources
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .extend(feed_sources);
        }

        self.load_venue_components_of(id, plugin.as_ref());

        self.static_plugin_gates
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), gate);
        self.late_activated_plugins
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(PluginRecord {
                plugin,
                manifest,
                activated_at: SystemTime::now(),
            });

        tracing::info!(plugin = id, "static plugin activated live, no restart");
        Ok(())
    }

    /// Every plugin this runtime knows about, static and package alike,
    /// presented in the one vocabulary the Plugins page reads —
    /// [`senken_plugin::ContributionKind`] badges for what each
    /// contributes, and one of a small set of user-facing states (see
    /// [`PluginListing`]).
    ///
    /// A package that contributes **both** a widget and a venue currently
    /// appears as two separate rows here (one per registry it was found
    /// in) rather than one merged row — the two registries key by
    /// different ids (a package's own directory id versus a venue
    /// component's own descriptor id) and nothing here yet proves the two
    /// are the same install. No shipped package declares both today.
    #[must_use]
    pub fn plugin_catalog(&self) -> Vec<PluginListing> {
        let mut out = self.static_catalog.clone();
        apply_live_plugin_gates(
            &mut out,
            &self
                .static_plugin_gates
                .read()
                .unwrap_or_else(PoisonError::into_inner),
        );
        if let Ok(packages) = self.widget_plugins.list() {
            for package in packages {
                // What the package declares, not what it currently serves: a disabled
                // package still is a venue, and a list filtered by kind must keep
                // showing it — its own toggle lives in that list.
                let contributes = package.declared.clone();
                let state = match package.status {
                    PackageStatus::Active => PluginListingState::Active,
                    PackageStatus::Disabled => PluginListingState::Disabled,
                    PackageStatus::Failed(reason) => PluginListingState::Failed(reason),
                };
                out.push(PluginListing {
                    id: package.id,
                    name: package.name,
                    version: package.version,
                    kind: PluginKind::Package,
                    contributes,
                    state,
                });
            }
        }
        for status in self.dynamic_venues.all() {
            let state = listing_state(status.state);
            // A venue this build compiles in serves its markets through its
            // own embedded component, and that component's descriptor id is
            // the market-data source id — not the plugin id. Look the owning
            // plugin up rather than matching ids, or the same venue appears
            // twice: once as the plugin a reader switches on, once as the
            // source it happens to serve under.
            let owners = self
                .static_venue_components
                .read()
                .unwrap_or_else(PoisonError::into_inner);
            let owner = owners
                .get(&status.id)
                .map_or(status.id.as_str(), String::as_str)
                .to_owned();
            drop(owners);
            let owner = owner.as_str();
            if let Some(existing) = out.iter_mut().find(|listing| listing.id == owner) {
                if !matches!(existing.state, PluginListingState::Active) {
                    existing.state = state;
                }
                continue;
            }
            out.push(PluginListing {
                id: status.id.clone(),
                name: status.name.clone().unwrap_or(status.id),
                version: String::new(),
                kind: PluginKind::Package,
                contributes: vec![ContributionKind::Venue],
                state,
            });
        }
        for status in self.dynamic_indicators.all() {
            let state = listing_state(status.state);
            let name = status
                .info
                .as_ref()
                .map_or_else(|| status.id.clone(), |info| info.title.clone());
            // Same reasoning as the venue loop above: an indicator that came
            // from a package is already listed under that package's id, and a
            // second row keyed the same way is indistinguishable from it.
            if let Some(existing) = out.iter_mut().find(|listing| listing.id == status.id) {
                if !existing.contributes.contains(&ContributionKind::Indicator) {
                    existing.contributes.push(ContributionKind::Indicator);
                }
                if !matches!(existing.state, PluginListingState::Active) {
                    existing.state = state;
                }
                continue;
            }
            out.push(PluginListing {
                id: status.id,
                name,
                version: String::new(),
                kind: PluginKind::Package,
                contributes: vec![ContributionKind::Indicator],
                state,
            });
        }
        debug_assert!(
            {
                let mut ids: Vec<&str> = out.iter().map(|listing| listing.id.as_str()).collect();
                ids.sort_unstable();
                let before = ids.len();
                ids.dedup();
                ids.len() == before
            },
            "a plugin id must appear once: callers key rows by it, and two rows \
             under one id cannot be told apart"
        );
        out
    }

    /// Every registered trade adapter, as one registry.
    ///
    /// Empty when no activated plugin registered one, which is a normal
    /// state: an installation that only reads market data has no adapters
    /// and needs none.
    #[must_use]
    pub fn trade(&self) -> &Arc<TradeEngine> {
        &self.trade
    }

    /// Deactivates every plugin in reverse order and consumes the runtime.
    /// Covers a plugin activated after startup through
    /// `Self::activate_stored_plugin` too — deactivated first, since it
    /// activated last — not just the ones `RuntimeBuilder::build` started.
    ///
    /// # Errors
    /// The first [`RuntimeError::PluginDeactivation`] encountered; the
    /// remaining plugins are still deactivated.
    pub fn shutdown(mut self) -> Result<(), RuntimeError> {
        deactivate_all(&mut self.all_activated_plugins())
    }

    /// `self.plugins` (activated at startup) followed by
    /// `self.late_activated_plugins` (activated afterward), combined into
    /// one `Vec` so [`deactivate_all`]'s own "reverse activation order"
    /// contract (it pops from the end) deactivates the late ones first —
    /// consistent with them having activated last. Takes `&mut self`
    /// rather than `&self`: both fields are moved out wholesale, which
    /// `Mutex::get_mut` can do lock-free precisely because a unique `&mut
    /// self` proves nothing else can be touching either field right now.
    fn all_activated_plugins(&mut self) -> Vec<PluginRecord> {
        let mut combined = std::mem::take(&mut self.plugins);
        combined.append(
            self.late_activated_plugins
                .get_mut()
                .unwrap_or_else(PoisonError::into_inner),
        );
        combined
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // After an explicit `shutdown` both lists are already empty.
        // Failures are logged inside `deactivate_all`; a drop site cannot
        // handle them.
        let _ = deactivate_all(&mut self.all_activated_plugins());
    }
}

#[cfg(test)]
mod tests {
    use super::{PluginListingState, Runtime, RuntimeError, StaticPluginToggleError};
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
                contributes: Vec::new(),
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
                    contributes: Vec::new(),
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
        // With an identity store attached, `plugin_state` is now the
        // source of truth for whether a static plugin activates at all
        // (see `DEFAULT_ENABLED_STATIC_PLUGINS`) — this test is about
        // permission reconciliation, not enablement, so it opts this
        // plugin in explicitly rather than relying on activating
        // unconditionally the way it could before that change landed.
        identity.set_plugin_enabled("test-plugin", true).unwrap();
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

    /// A minimal static venue plugin for the default-enable tests below —
    /// registers a `MarketDataSource` under its own id, nothing else.
    struct VenuePlugin(&'static str);

    #[async_trait::async_trait]
    impl senken_marketdata::MarketDataSource for VenuePlugin {
        fn id(&self) -> &str {
            self.0
        }
        fn name(&self) -> &str {
            self.0
        }
        async fn instruments(
            &self,
        ) -> Result<Vec<senken_marketdata::Instrument>, senken_marketdata::SourceError> {
            Ok(Vec::new())
        }
    }

    impl Plugin for VenuePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.0.to_string(),
                name: self.0.to_string(),
                version: "0".into(),
                description: String::new(),
                permissions: Vec::new(),
                contributes: vec![senken_plugin::ContributionKind::Venue],
            }
        }

        fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
            context.register_marketdata_source(Arc::new(VenuePlugin(self.0)));
            Ok(())
        }
    }

    /// A static plugin that has moved its venue contribution to a
    /// component, the same shape `senken_plugin_okx::OkxPlugin` is now —
    /// `venue_components` returns bytes rather than an empty `Vec`, and
    /// `activate` registers nothing of its own (matching `OkxPlugin` no
    /// longer registering a native source for whatever its components now
    /// serve).
    struct VenuePluginWithComponent {
        id: &'static str,
        component: &'static [u8],
    }

    impl Plugin for VenuePluginWithComponent {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.id.to_string(),
                name: self.id.to_string(),
                version: "0".into(),
                description: String::new(),
                permissions: Vec::new(),
                contributes: vec![senken_plugin::ContributionKind::Venue],
            }
        }

        fn activate(&self, _context: &mut ActivationContext) -> Result<(), PluginError> {
            Ok(())
        }

        fn venue_components(&self) -> Vec<&'static [u8]> {
            vec![self.component]
        }
    }

    /// The property `senken_plugin_okx::OkxPlugin::venue_components` and
    /// `load_static_venue_components` exist for: an activated static
    /// plugin's embedded component is registered into the same dynamic-
    /// venue registry a package's own `venue` contribution uses, under
    /// [`super::plugin_host::PluginOrigin::BuiltIn`] specifically — the
    /// origin the Plugins page uses to tell "ships with Senken" apart from
    /// "installed from a package". The bytes here are not a loadable
    /// component (a real one needs a genuine `wasm32-wasip2` build, which
    /// this crate's own tests do not compile — `plugins/okx`'s own
    /// `tests/wasm_parity.rs` proves the real component instead); this
    /// test is only about whether the runtime *attempts* the registration
    /// with the right origin, which happens whether the load succeeds or
    /// not (see `DynamicVenues::register_with_origin_and_base_url`'s own
    /// docs on recording a load failure as a visible catalog entry).
    #[test]
    fn a_static_plugins_embedded_venue_component_registers_with_the_builtin_origin() {
        // A data directory of its own, like every other test here: without
        // one the builder falls back to `.data` relative to the working
        // directory, which for a unit test is the crate's own source tree.
        let dir = tempfile::TempDir::new().unwrap();
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .plugin(VenuePluginWithComponent {
                id: "has-a-component",
                component: b"not a real wasm32-wasip2 component",
            })
            .build()
            .unwrap();

        let statuses = runtime.dynamic_venues().all();
        let entry = statuses
            .iter()
            .find(|status| status.id.starts_with("FailedVenue-") || status.id == "has-a-component")
            .expect("the embedded component must produce some dynamic-venue catalog entry");
        assert_eq!(
            entry.origin,
            crate::plugin_host::PluginOrigin::BuiltIn,
            "a static plugin's own embedded component must register as BuiltIn, not Uploaded/DataDirectory"
        );
    }

    #[test]
    fn a_fresh_install_activates_only_the_seeded_default_plugins_and_lists_the_rest_disabled() {
        let dir = TempDir::new().unwrap();
        let identity = Arc::new(IdentityStore::open(dir.path().join("accounts.db")).unwrap());

        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .identity_store(Arc::clone(&identity))
            .plugin(VenuePlugin("okx"))
            .plugin(VenuePlugin("simulator"))
            .plugin(VenuePlugin("bybit"))
            .build()
            .unwrap();

        let catalog = runtime.plugin_catalog();
        let state_of = |id: &str| {
            catalog
                .iter()
                .find(|listing| listing.id == id)
                .unwrap_or_else(|| panic!("{id} must still be listed even when disabled"))
                .state
                .clone()
        };
        assert_eq!(state_of("okx"), super::PluginListingState::Active);
        assert_eq!(state_of("simulator"), super::PluginListingState::Active);
        assert_eq!(
            state_of("bybit"),
            super::PluginListingState::Disabled,
            "a plugin outside the seeded default set must not activate on a fresh install"
        );

        let source_ids: Vec<String> = runtime
            .marketdata()
            .sources()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert!(source_ids.contains(&"okx".to_owned()));
        assert!(source_ids.contains(&"simulator".to_owned()));
        assert!(
            !source_ids.contains(&"bybit".to_owned()),
            "a disabled plugin's source must never reach the market data catalog"
        );

        runtime.shutdown().unwrap();
    }

    #[test]
    fn a_static_plugin_disabled_in_plugin_state_is_listed_but_not_activated() {
        let dir = TempDir::new().unwrap();
        let identity = Arc::new(IdentityStore::open(dir.path().join("accounts.db")).unwrap());
        // Seed first (as a real startup would), then override one plugin's
        // decision explicitly — proving the table, not just the seed, is
        // what `build()` reads.
        identity
            .seed_default_plugin_state(&["okx", "simulator"])
            .unwrap();
        identity.set_plugin_enabled("okx", false).unwrap();

        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .identity_store(Arc::clone(&identity))
            .plugin(VenuePlugin("okx"))
            .build()
            .unwrap();

        let catalog = runtime.plugin_catalog();
        let okx = catalog
            .iter()
            .find(|listing| listing.id == "okx")
            .expect("a disabled static plugin must still appear in the catalog");
        assert_eq!(okx.state, super::PluginListingState::Disabled);
        assert!(
            runtime.plugins().is_empty(),
            "a plugin an admin explicitly disabled must never have been activated"
        );
        assert!(
            !runtime
                .marketdata()
                .sources()
                .into_iter()
                .any(|s| s.id == "okx"),
            "a disabled plugin's source must not appear in the market data catalog"
        );

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
                    contributes: Vec::new(),
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

    /// A static venue plugin whose catalog is never empty while enabled —
    /// unlike [`VenuePlugin`] above (built for tests that only care whether
    /// a source reaches the registry at all), this one has to actually
    /// prove something disappears and comes back.
    struct RichVenuePlugin(&'static str);

    #[async_trait::async_trait]
    impl senken_marketdata::MarketDataSource for RichVenuePlugin {
        fn id(&self) -> &str {
            self.0
        }
        fn name(&self) -> &str {
            self.0
        }
        async fn instruments(
            &self,
        ) -> Result<Vec<senken_marketdata::Instrument>, senken_marketdata::SourceError> {
            Ok(vec![senken_marketdata::Instrument::spot(
                "BTCUSDT", "BTC-USDT", "BTC", "USDT",
            )])
        }
    }

    impl Plugin for RichVenuePlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.0.to_string(),
                name: self.0.to_string(),
                version: "0".into(),
                description: String::new(),
                permissions: Vec::new(),
                contributes: vec![senken_plugin::ContributionKind::Venue],
            }
        }

        fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
            context.register_marketdata_source(Arc::new(RichVenuePlugin(self.0)));
            Ok(())
        }
    }

    /// Required test 1 of the live-plugin-toggle work: disabling a static
    /// plugin must empty its catalog immediately, and enabling it again must
    /// restore it — with no rebuild of the `Runtime` in between, the same
    /// property `plugin_host::DynamicVenues` already has for a package.
    #[tokio::test]
    async fn disabling_a_static_plugin_live_empties_its_catalog_and_enabling_restores_it_with_no_rebuild()
     {
        let dir = TempDir::new().unwrap();
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .plugin(RichVenuePlugin("rich-venue"))
            .build()
            .unwrap();

        // `refresh` bypasses `MarketData`'s own cache — without it, a
        // second query inside the cache's TTL would read back the first
        // answer regardless of what the source now reports, proving
        // nothing about the live toggle this test is about.
        let before = runtime.marketdata().refresh("rich-venue").await.unwrap();
        assert_eq!(before.instrument_count, 1);

        runtime
            .set_static_plugin_enabled("rich-venue", false)
            .unwrap();
        let disabled = runtime.marketdata().refresh("rich-venue").await.unwrap();
        assert_eq!(
            disabled.instrument_count, 0,
            "disabling the plugin must empty its catalog immediately, no rebuild"
        );
        let catalog = runtime.plugin_catalog();
        assert_eq!(
            catalog
                .iter()
                .find(|listing| listing.id == "rich-venue")
                .unwrap()
                .state,
            PluginListingState::Disabled,
            "the catalog listing itself must report the live state honestly"
        );

        runtime
            .set_static_plugin_enabled("rich-venue", true)
            .unwrap();
        let restored = runtime.marketdata().refresh("rich-venue").await.unwrap();
        assert_eq!(
            restored.instrument_count, 1,
            "re-enabling the plugin must restore its catalog immediately, no rebuild"
        );

        runtime.shutdown().unwrap();
    }

    /// A minimal plugin registering only a [`senken_trade::TradeAdapter`] —
    /// standing in for `simulator`, the one shipped plugin shaped this way
    /// today.
    struct TradeOnlyPlugin(&'static str);

    struct NoopAdapter(&'static str);

    #[async_trait::async_trait]
    impl senken_trade::TradeAdapter for NoopAdapter {
        fn id(&self) -> &'static str {
            "trade-only"
        }
        fn name(&self) -> &'static str {
            self.0
        }
        fn kind(&self) -> senken_trade::AdapterKind {
            senken_trade::AdapterKind::Simulation
        }
        fn capabilities(&self) -> senken_trade::AdapterCapabilities {
            senken_trade::AdapterCapabilities::market_only()
        }
        fn coverage(&self) -> senken_trade::InstrumentCoverage {
            senken_trade::InstrumentCoverage::Universal
        }
        fn settings_schema(&self) -> senken_trade::SettingsSchema {
            senken_trade::SettingsSchema::default()
        }
        async fn open_account(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
        ) -> Result<(), senken_trade::TradeError> {
            Ok(())
        }
        async fn health(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
        ) -> Result<senken_trade::AdapterHealth, senken_trade::TradeError> {
            Ok(senken_trade::AdapterHealth::Connected)
        }
        async fn balances(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            account: senken_trade::AccountRef<'_>,
        ) -> Result<senken_trade::AccountBalances, senken_trade::TradeError> {
            Ok(senken_trade::AccountBalances {
                account_id: account.id,
                currency: "USD".to_owned(),
                balance: senken_core::decimal::Scaled::new(2, 0),
                equity: senken_core::decimal::Scaled::new(2, 0),
                unrealized_pnl: senken_core::decimal::Scaled::new(2, 0),
                margin_used: None,
                margin_available: None,
                assets: Vec::new(),
            })
        }
        async fn positions(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
        ) -> Result<Vec<senken_trade::Position>, senken_trade::TradeError> {
            Ok(Vec::new())
        }
        async fn orders(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
            _filter: senken_trade::OrderFilter,
        ) -> Result<Vec<senken_trade::Order>, senken_trade::TradeError> {
            Ok(Vec::new())
        }
        async fn place_order(
            &self,
            _ctx: &senken_trade::TradeContext<'_>,
            _account: senken_trade::AccountRef<'_>,
            _request: senken_trade::OrderRequest,
        ) -> Result<senken_trade::Order, senken_trade::TradeError> {
            Err(senken_trade::TradeError::unsupported(
                "trade-only",
                "trading",
            ))
        }
    }

    impl Plugin for TradeOnlyPlugin {
        fn manifest(&self) -> PluginManifest {
            PluginManifest {
                id: self.0.to_string(),
                name: self.0.to_string(),
                version: "0".into(),
                description: String::new(),
                permissions: Vec::new(),
                contributes: vec![senken_plugin::ContributionKind::TradeAdapter],
            }
        }

        fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
            context.register_trade_adapter(Arc::new(NoopAdapter(self.0)));
            Ok(())
        }
    }

    /// The scope decision this work made explicit: a registered trade
    /// adapter is never turned off live. Disabling a trade-only plugin must
    /// leave its adapter fully reachable, and the catalog must keep saying
    /// so — a listing that claimed "disabled" here would be lying about
    /// money still being able to move through it.
    #[test]
    fn a_plugin_with_a_trade_adapter_is_never_gated_live_and_keeps_needing_a_restart() {
        let dir = TempDir::new().unwrap();
        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .plugin(TradeOnlyPlugin("trade-only"))
            .build()
            .unwrap();

        runtime
            .set_static_plugin_enabled("trade-only", false)
            .unwrap();

        assert!(
            runtime.trade().adapter("trade-only").is_ok(),
            "a trade adapter must stay reachable — this project never turns one off live"
        );
        let catalog = runtime.plugin_catalog();
        assert_eq!(
            catalog
                .iter()
                .find(|listing| listing.id == "trade-only")
                .unwrap()
                .state,
            PluginListingState::Active,
            "a plugin whose adapter still trades must never report itself disabled"
        );

        runtime.shutdown().unwrap();
    }

    /// A plugin declaring a trade adapter is the one case that still
    /// genuinely needs a restart to enable, on purpose (see
    /// `Runtime::activate_stored_plugin`'s own docs) — `TradeEngine` is
    /// shared behind an `Arc` once the server is running, so there is no
    /// unique `&mut TradeEngine` this call could ever hand a late
    /// activation, whatever this project decided about the scope question.
    /// `set_static_plugin_enabled` must say so rather than silently
    /// accepting a flip it cannot apply.
    #[test]
    fn a_plugin_declaring_a_trade_adapter_still_needs_a_restart_to_enable_it() {
        let dir = TempDir::new().unwrap();
        let identity = Arc::new(IdentityStore::open(dir.path().join("accounts.db")).unwrap());
        identity
            .set_plugin_enabled("late-trade-only", false)
            .unwrap();

        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .identity_store(Arc::clone(&identity))
            .plugin(TradeOnlyPlugin("late-trade-only"))
            .build()
            .unwrap();

        assert!(
            runtime.plugins().is_empty(),
            "the plugin must never have activated"
        );
        assert!(matches!(
            runtime.set_static_plugin_enabled("late-trade-only", true),
            Err(StaticPluginToggleError::NeverActivated(ref id)) if id == "late-trade-only"
        ));
        // Disabling a plugin that is already off this way is a no-op, not
        // an error — it is already exactly what was asked for.
        assert!(
            runtime
                .set_static_plugin_enabled("late-trade-only", false)
                .is_ok()
        );
        assert!(matches!(
            runtime.set_static_plugin_enabled("does-not-exist", false),
            Err(StaticPluginToggleError::UnknownPlugin(ref id)) if id == "does-not-exist"
        ));

        runtime.shutdown().unwrap();
    }

    /// Required test 1 of the never-activated-plugin live-toggle work:
    /// turning on a plugin that never activated at boot must make its
    /// instruments appear with no rebuild of the `Runtime`, and turning it
    /// back off must make them disappear again — the same property
    /// `disabling_a_static_plugin_live_empties_its_catalog_and_enabling_restores_it_with_no_rebuild`
    /// already proves for a plugin that *was* active at boot, extended to
    /// one that never got that far. This test creates the state itself:
    /// the plugin is built disabled, its source is proven entirely absent
    /// from the catalog (not merely empty — it was never registered at
    /// all), then enabled live, then disabled again.
    #[tokio::test]
    async fn enabling_a_plugin_that_never_activated_at_boot_makes_its_instruments_appear_with_no_rebuild()
     {
        let dir = TempDir::new().unwrap();
        let identity = Arc::new(IdentityStore::open(dir.path().join("accounts.db")).unwrap());
        identity.set_plugin_enabled("late-venue", false).unwrap();

        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .identity_store(Arc::clone(&identity))
            .plugin(RichVenuePlugin("late-venue"))
            .build()
            .unwrap();

        assert!(
            runtime.plugins().is_empty(),
            "a plugin an admin disabled before boot must never activate at startup"
        );
        let ids_before: Vec<String> = runtime
            .marketdata()
            .sources()
            .into_iter()
            .map(|s| s.id)
            .collect();
        assert!(
            !ids_before.contains(&"late-venue".to_owned()),
            "an un-activated plugin's source must not exist in the catalog at all, \
             not merely read back empty"
        );

        runtime
            .set_static_plugin_enabled("late-venue", true)
            .unwrap();

        let detail = runtime.marketdata().refresh("late-venue").await.unwrap();
        assert_eq!(
            detail.instrument_count, 1,
            "turning the plugin on live must make its instruments appear with no restart"
        );
        assert_eq!(
            runtime
                .plugin_catalog()
                .into_iter()
                .find(|listing| listing.id == "late-venue")
                .unwrap()
                .state,
            PluginListingState::Active
        );

        runtime
            .set_static_plugin_enabled("late-venue", false)
            .unwrap();
        let disabled = runtime.marketdata().refresh("late-venue").await.unwrap();
        assert_eq!(
            disabled.instrument_count, 0,
            "disabling it again must empty its catalog immediately too"
        );

        runtime.shutdown().unwrap();
    }

    /// Required test 2: turning on a plugin that is already active — at
    /// boot, or from a previous live activation this same run — must never
    /// register a second source. Going through the public
    /// `set_static_plugin_enabled` entry point twice in a row (the second
    /// call finds the gate already there and only flips the atomic flag,
    /// never re-running activation) is the ordinary path a caller takes;
    /// [`activating_the_same_stored_plugin_twice_directly_is_refused_not_double_registered`]
    /// below proves the guarantee holds even when that ordinary short
    /// circuit is bypassed.
    #[tokio::test]
    async fn enabling_an_already_active_plugin_again_does_not_register_a_second_source() {
        let dir = TempDir::new().unwrap();
        let identity = Arc::new(IdentityStore::open(dir.path().join("accounts.db")).unwrap());
        identity.set_plugin_enabled("late-venue-2", false).unwrap();

        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .identity_store(Arc::clone(&identity))
            .plugin(RichVenuePlugin("late-venue-2"))
            .build()
            .unwrap();

        runtime
            .set_static_plugin_enabled("late-venue-2", true)
            .unwrap();
        assert_eq!(runtime.marketdata().sources().len(), 1);

        runtime
            .set_static_plugin_enabled("late-venue-2", true)
            .unwrap();
        assert_eq!(
            runtime.marketdata().sources().len(),
            1,
            "enabling an already-active plugin again must not register a second source"
        );
        let page = runtime.marketdata().refresh("late-venue-2").await.unwrap();
        assert_eq!(
            page.instrument_count, 1,
            "and must not duplicate that source's own instrument either"
        );

        runtime.shutdown().unwrap();
    }

    /// The structural half of required test 2: calling the low-level
    /// activation directly a second time — bypassing
    /// `set_static_plugin_enabled`'s own "a gate already exists" short
    /// circuit on purpose — must still be refused, because the plugin was
    /// already drained out of `stored_static_plugins` by the first call.
    /// Dropping that drain (changing it to a non-removing lookup) makes
    /// this test fail: the second call would go on to call
    /// `MarketData::register_source` for the same id again, which is
    /// exactly the double registration this guarantee exists to make
    /// impossible rather than merely unlikely.
    #[test]
    fn activating_the_same_stored_plugin_twice_directly_is_refused_not_double_registered() {
        let dir = TempDir::new().unwrap();
        let identity = Arc::new(IdentityStore::open(dir.path().join("accounts.db")).unwrap());
        identity.set_plugin_enabled("late-venue-3", false).unwrap();

        let runtime = Runtime::builder()
            .data_dir(dir.path())
            .identity_store(Arc::clone(&identity))
            .plugin(RichVenuePlugin("late-venue-3"))
            .build()
            .unwrap();

        runtime.activate_stored_plugin("late-venue-3").unwrap();
        assert_eq!(runtime.marketdata().sources().len(), 1);

        assert!(matches!(
            runtime.activate_stored_plugin("late-venue-3"),
            Err(StaticPluginToggleError::NeverActivated(ref id)) if id == "late-venue-3"
        ));
        assert_eq!(
            runtime.marketdata().sources().len(),
            1,
            "a second activation attempt must not register a second source"
        );

        runtime.shutdown().unwrap();
    }
}
