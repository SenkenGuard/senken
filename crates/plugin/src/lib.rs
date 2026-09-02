//! The contract a Senken plugin implements.
//!
//! A plugin is a unit of integration: it describes itself with a
//! [`PluginManifest`] and, on [`activate`](Plugin::activate), registers the
//! capabilities it provides into an [`ActivationContext`]. The runtime owns
//! the lifecycle; a plugin never talks to other plugins directly. The
//! context also provisions shared infrastructure — today one HTTP client —
//! so plugins do not each build their own.
//!
//! Four capabilities exist today: [`MarketDataSource`] (instruments),
//! [`BarSource`] (bars), [`BookSource`] (order-book depth) and
//! [`FeedSource`] (a live price/quote stream). A plugin registering them
//! for one venue
//! must share a single [`LimitGroup`] between them (obtain it once from an
//! [`HttpActivationContext`]) — instrument and
//! bar traffic drawing independent budgets against one real venue quota is
//! not safe. The context caches each named handle so the two sources use the
//! same budget.
//!
//! Library consumers who only want, say, a market data source do not need
//! this crate: every capability a plugin registers is an ordinary type from
//! its own domain crate that can be used on its own.
//!
//! # Design: the context is concrete on purpose
//!
//! [`ActivationContext`] names each capability as a typed field and method
//! (today only [`register_marketdata_source`]) instead of hiding them behind
//! a type-erased map. The cost is a dependency from this crate on every
//! domain crate it can register, and it is paid deliberately: registration
//! is discoverable in rustdoc, checked at compile time, and all Senken
//! crates currently version in lockstep anyway.
//!
//! Revisit this when a second domain crate arrives, or when plugins need to
//! release independently of the domain crates. The exit is a type-map
//! context (`TypeId` → erased registry) with one extension trait per domain
//! crate, which inverts the dependency so each domain defines its own
//! registration — trading away compile-time checking and discoverability.
//!
//! [`register_marketdata_source`]: ActivationContext::register_marketdata_source
//!
//! # Cargo features
//!
//! * `http` *(default)* — [`HttpActivationContext::limit_group`] and
//!   [`HttpActivationContext::venue_client`], and with them `reqwest`,
//!   `senken-venue` and a TLS stack. A plugin that talks to no network — one
//!   reading local files, say — should take this crate with
//!   `default-features = false`; a venue plugin should ask for
//!   `features = ["http"]` explicitly rather than rely on someone else
//!   leaving the default on.

use std::fmt;
use std::sync::Arc;
#[cfg(feature = "http")]
use std::{collections::HashMap, time::Duration};

use senken_acl::{PluginNamespace, PluginPermissionError, PluginPermissionName};
use senken_marketdata::book::BookSource;
use senken_marketdata::source::MarketDataSource;
use senken_subscription::FeedSource;
#[cfg(feature = "http")]
use senken_venue::{LimitGroup, VenueClient};

/// The bar-fetching contract.
pub mod bar_source;
/// The wall clock a venue adapter closes candles against.
pub mod clock;
/// Error types.
pub mod error;

/// Shared decoding helpers for a venue's live feed.
pub mod live;
/// Reconciling a plugin's permissions across activations.
pub mod permissions;

pub use crate::bar_source::BarSource;
pub use crate::clock::SystemClock;
pub use crate::error::{BoxError, PluginError, PluginPermissionRegistrationError};
pub use crate::permissions::reconcile_plugin_permissions;

/// Static facts about a plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginManifest {
    /// Stable identifier, unique within a runtime (`binance-spot`). Also the
    /// plugin's permission namespace: permissions this
    /// manifest declares, or that the plugin registers at runtime through
    /// [`ActivationContext::register_plugin_permission`], must live under
    /// `<id>.<resource>:<operation>`. See
    /// [`permission_namespace`](Self::permission_namespace).
    pub id: String,
    /// Display name.
    pub name: String,
    /// The plugin's own version string.
    pub version: String,
    /// One-line description.
    pub description: String,
    /// Permissions this plugin ships with at build time, within the
    /// namespace `id` delegates. Most plugins have none — a
    /// plugin with no permission-gated actions (every venue plugin today)
    /// declares an empty list; the moment a name is added here, an admin
    /// can assign it to a role. A plugin that cannot know its permissions
    /// until runtime — one mirroring an external system's resources —
    /// leaves this empty and calls
    /// [`ActivationContext::register_plugin_permission`] from
    /// [`Plugin::activate`] instead.
    pub permissions: Vec<PluginPermissionName>,
}

impl PluginManifest {
    /// The namespace `id` delegates authority over — the
    /// manifest's own subtree, like a DNS zone, and the only namespace this
    /// plugin may declare or register permissions under.
    ///
    /// # Errors
    /// [`PluginPermissionError`] if `id` is not a valid namespace slug
    /// (non-empty, lowercase ASCII letters, digits and `-`).
    pub fn permission_namespace(&self) -> Result<PluginNamespace, PluginPermissionError> {
        PluginNamespace::new(&self.id)
    }

    /// Checks that every entry of [`permissions`](Self::permissions)
    /// actually belongs to this manifest's own namespace — the build-time
    /// twin of [`ActivationContext::register_plugin_permission`]'s runtime
    /// check.
    ///
    /// A manifest is hand-written data, so nothing stops a typo from naming
    /// a foreign namespace directly in the list; this is what turns that
    /// typo into a reported error instead of a silently mis-scoped
    /// permission slipping in compiled and trusted.
    ///
    /// # Errors
    /// [`PluginPermissionError`] when `id` is not a valid namespace, or an
    /// entry of `permissions` names a different one.
    pub fn validate_permissions(&self) -> Result<(), PluginPermissionError> {
        let namespace = self.permission_namespace()?;
        for permission in &self.permissions {
            namespace.admit(permission.clone())?;
        }
        Ok(())
    }
}

#[cfg(feature = "http")]
const HTTP_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(feature = "http")]
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// What a plugin contributes during activation, and what activation
/// provides to a plugin.
///
/// Every method takes `&mut self`; the runtime drains the registrations
/// after each plugin's [`Plugin::activate`] returns, reusing one context —
/// and the shared resources it caches — across every plugin.
#[derive(Debug, Default)]
pub struct ActivationContext {
    marketdata_sources: Vec<Arc<dyn MarketDataSource>>,
    bar_sources: Vec<Arc<dyn BarSource>>,
    book_sources: Vec<Arc<dyn BookSource>>,
    feed_sources: Vec<Arc<dyn FeedSource>>,
    #[cfg(feature = "http")]
    http_client: Option<reqwest::Client>,
    /// Groups already handed out this context's lifetime, keyed by the name
    /// passed to [`limit_group`](Self::limit_group). See that method's docs
    /// for why caching the handle here — rather than building a fresh
    /// [`LimitGroup`] every call — is what makes `name` mean anything.
    #[cfg(feature = "http")]
    limit_groups: HashMap<String, LimitGroup>,
    /// The namespace the runtime bound before calling the plugin currently
    /// activating, via [`bind_permission_namespace`](Self::bind_permission_namespace).
    /// `None` until bound, and again after
    /// [`take_plugin_permissions`](Self::take_plugin_permissions) drains it —
    /// a plugin can only ever register into whatever the runtime bound for
    /// *this* activation, never a namespace of its own choosing.
    current_namespace: Option<PluginNamespace>,
    /// Permissions registered since the last
    /// [`take_plugin_permissions`](Self::take_plugin_permissions).
    plugin_permissions: Vec<PluginPermissionName>,
}

/// Activation context for a module that explicitly declares HTTP access.
///
/// Network methods live only on this type. A module activated through
/// [`Plugin::activate_without_io`] receives [`ActivationContext`] instead,
/// so attempting to use HTTP there is a compile-time error.
pub struct HttpActivationContext<'a> {
    inner: &'a mut ActivationContext,
}

impl fmt::Debug for HttpActivationContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpActivationContext")
            .finish_non_exhaustive()
    }
}

/// The proactive budget every venue group starts with, and the window it
/// is measured over.
///
/// **Not a venue-documented rate limit** — no venue this build talks to
/// returns a rate-limit weight in a response header for the endpoints it
/// uses. This is a conservative ceiling this project imposes on itself,
/// deliberately well under anything a public market endpoint is likely to
/// allow. Against the per-call costs the venue adapters charge, it admits
/// roughly ten requests a second per venue.
///
/// A *default*, and that is the point. A group used only for one-shot
/// fetches barely notices it, but order-book depth is now **polled** —
/// once a second per watched instrument — and a plugin author cannot be
/// expected to remember that when they write a book source. Leaving the
/// ceiling to be set per plugin is a convention, and a convention is what
/// gets forgotten: this project has already had one venue ban an IP.
/// A plugin that knows better can still widen or narrow it with
/// [`LimitGroup::per_window`], which composes rather than replaces.
#[cfg(feature = "http")]
const DEFAULT_LIMIT_WINDOW: Duration = Duration::from_secs(1);
/// See [`DEFAULT_LIMIT_WINDOW`].
#[cfg(feature = "http")]
const DEFAULT_LIMIT_BUDGET: u32 = 50;

impl HttpActivationContext<'_> {
    /// Contributes a market data source.
    pub fn register_marketdata_source(&mut self, source: Arc<dyn MarketDataSource>) {
        self.inner.register_marketdata_source(source);
    }

    /// Contributes a bar source.
    pub fn register_bar_source(&mut self, source: Arc<dyn BarSource>) {
        self.inner.register_bar_source(source);
    }

    /// Contributes an order-book depth source.
    pub fn register_book_source(&mut self, source: Arc<dyn BookSource>) {
        self.inner.register_book_source(source);
    }

    /// Contributes a live-feed source.
    pub fn register_feed_source(&mut self, source: Arc<dyn FeedSource>) {
        self.inner.register_feed_source(source);
    }

    /// Registers a permission in the namespace bound by the runtime.
    ///
    /// # Errors
    /// See [`ActivationContext::register_plugin_permission`].
    pub fn register_plugin_permission(
        &mut self,
        name: PluginPermissionName,
    ) -> Result<(), PluginPermissionRegistrationError> {
        self.inner.register_plugin_permission(name)
    }
}

impl ActivationContext {
    /// An empty context.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Contributes a market data source.
    pub fn register_marketdata_source(&mut self, source: Arc<dyn MarketDataSource>) {
        self.marketdata_sources.push(source);
    }

    /// Takes the sources registered since the last call, leaving the
    /// context — and the resources it caches — ready for the next plugin.
    #[must_use]
    pub fn take_marketdata_sources(&mut self) -> Vec<Arc<dyn MarketDataSource>> {
        std::mem::take(&mut self.marketdata_sources)
    }

    /// Contributes a bar source, mirroring
    /// [`register_marketdata_source`](Self::register_marketdata_source)
    /// exactly.
    pub fn register_bar_source(&mut self, source: Arc<dyn BarSource>) {
        self.bar_sources.push(source);
    }

    /// Takes the bar sources registered since the last call, leaving the
    /// context ready for the next plugin.
    #[must_use]
    pub fn take_bar_sources(&mut self) -> Vec<Arc<dyn BarSource>> {
        std::mem::take(&mut self.bar_sources)
    }

    /// Contributes an order-book depth source, mirroring
    /// [`register_bar_source`](Self::register_bar_source) exactly.
    ///
    /// Registration *is* the capability declaration, the same way it is for
    /// instruments and bars: a venue that serves no depth registers nothing
    /// here, and whatever reports capabilities to a client reads that
    /// absence rather than guessing (see
    /// [`BookSource`]'s own docs on why
    /// there is deliberately no default returning an empty book).
    ///
    /// Depth is stateless to fetch — a client and a URL — so a plugin can
    /// build one during activation, unlike a live feed, which cannot exist
    /// until the instrument catalog every plugin is still registering into
    /// has been assembled.
    pub fn register_book_source(&mut self, source: Arc<dyn BookSource>) {
        self.book_sources.push(source);
    }

    /// Takes the book sources registered since the last call, leaving the
    /// context ready for the next plugin.
    #[must_use]
    pub fn take_book_sources(&mut self) -> Vec<Arc<dyn BookSource>> {
        std::mem::take(&mut self.book_sources)
    }

    /// Contributes a live-feed source.
    ///
    /// A [`FeedSource`] rather than a finished protocol, and that
    /// difference is the whole reason this method looks unlike its three
    /// siblings: a live protocol cannot exist during activation, because it
    /// needs an instrument catalog that activation is still assembling. See
    /// [`FeedSource`]'s own docs.
    pub fn register_feed_source(&mut self, source: Arc<dyn FeedSource>) {
        self.feed_sources.push(source);
    }

    /// Takes the feed sources registered since the last call, leaving the
    /// context ready for the next plugin.
    #[must_use]
    pub fn take_feed_sources(&mut self) -> Vec<Arc<dyn FeedSource>> {
        std::mem::take(&mut self.feed_sources)
    }

    /// Binds `namespace` as the only namespace
    /// [`register_plugin_permission`](Self::register_plugin_permission) will
    /// admit until the next [`take_plugin_permissions`](Self::take_plugin_permissions).
    ///
    /// The runtime calls this immediately before [`Plugin::activate`], with
    /// the namespace [`PluginManifest::permission_namespace`] computes from
    /// the manifest it already trusts — a plugin is never asked for, and
    /// never supplies, its own namespace here, only what it names inside
    /// it.
    pub fn bind_permission_namespace(&mut self, namespace: PluginNamespace) {
        self.current_namespace = Some(namespace);
    }

    /// Registers that `name` exists, so an admin can later assign it to a
    /// role. **This never grants anything, to anyone,
    /// including the calling plugin** — there is no function anywhere in
    /// this crate, or in `senken_acl`, that turns a
    /// [`PluginPermissionName`] into a [`senken_acl::Grant`]; naming a
    /// permission and assigning it are different operations, and this one
    /// only ever does the former.
    ///
    /// # Errors
    /// [`PluginPermissionRegistrationError::NoNamespaceBound`] if no
    /// namespace has been bound for this activation yet (see
    /// [`bind_permission_namespace`](Self::bind_permission_namespace)), or
    /// [`PluginPermissionRegistrationError::InvalidPermission`] if `name`
    /// does not belong to the bound namespace — the manifest delegates
    /// authority over its own subtree only, like a DNS zone, and a plugin
    /// cannot register, say, `senken.users:manage`.
    pub fn register_plugin_permission(
        &mut self,
        name: PluginPermissionName,
    ) -> Result<(), PluginPermissionRegistrationError> {
        let namespace = self
            .current_namespace
            .as_ref()
            .ok_or(PluginPermissionRegistrationError::NoNamespaceBound)?;
        let admitted = namespace.admit(name)?;
        self.plugin_permissions.push(admitted);
        Ok(())
    }

    /// Takes the permissions registered since the last call and un-binds
    /// the namespace, leaving the context ready for the next plugin's
    /// [`bind_permission_namespace`](Self::bind_permission_namespace) call —
    /// exactly like [`take_bar_sources`](Self::take_bar_sources) does for
    /// bar sources, plus clearing the namespace so a permission cannot be
    /// registered between plugins with no namespace bound at all.
    #[must_use]
    pub fn take_plugin_permissions(&mut self) -> Vec<PluginPermissionName> {
        self.current_namespace = None;
        std::mem::take(&mut self.plugin_permissions)
    }
}

impl HttpActivationContext<'_> {
    /// An HTTP client shared by every HTTP-capable module.
    ///
    /// # Errors
    /// [`PluginError`] when the TLS backend cannot initialise.
    #[cfg(feature = "http")]
    pub fn http_client(&mut self) -> Result<reqwest::Client, PluginError> {
        self.inner.shared_http_client()
    }

    /// A [`LimitGroup`] named `name`, holding the rate, concurrency and
    /// failure budget one venue should share across every source it
    /// registers — `binance-spot`, `binance-usdm` and `binance-coinm` must
    /// draw from one Binance group, not three.
    ///
    /// **Cached by name on the context**, and this is load-bearing, not an
    /// optimisation: onward a single plugin registers both a
    /// `MarketDataSource` and a `BarSource` for one venue, and each is free
    /// to call `limit_group("binance")` independently while building its own
    /// client. If every call built a fresh, unconnected [`LimitGroup`], the
    /// two kinds of traffic would spend two independent budgets against one
    /// real venue quota — precisely the failure D15 introduced limit groups
    /// to prevent, reappearing one level up. Two
    /// calls with the same `name` now return clones that share one
    /// underlying group, exactly like `binance-spot` and `binance-usdm`
    /// already share one group via explicit `clone()` today.
    ///
    /// This is safe even though [`LimitGroup::per_window`] and
    /// [`LimitGroup::max_concurrent`] are consuming builders: both mutate
    /// state held behind the group's internal `Arc` and hand the same value
    /// back, rather than producing an unrelated new group. So a handle
    /// cloned out of the cache *before* another call site configures it
    /// still observes that configuration afterwards — there is no "the
    /// first configured group I built was thrown away" failure mode to
    /// design around here, which is what would make a plain
    /// name-to-fresh-group cache wrong. (A venue that instead needed
    /// configuration to run exactly once — say, a step whose side effect is
    /// not idempotent — would need `limit_group_with(name, |g| ..)` gated on
    /// first creation instead; nothing here needs that yet.)
    #[cfg(feature = "http")]
    #[must_use]
    pub fn limit_group(&mut self, name: &str) -> LimitGroup {
        self.inner
            .limit_groups
            .entry(name.to_owned())
            .or_insert_with(|| {
                LimitGroup::new(name).per_window(DEFAULT_LIMIT_WINDOW, DEFAULT_LIMIT_BUDGET)
            })
            .clone()
    }

    /// A [`VenueClient`] fetching through this context's shared
    /// [`reqwest::Client`], gated by `group`.
    ///
    /// # Errors
    /// [`PluginError`] when the underlying HTTP client cannot be built —
    /// see [`http_client`](Self::http_client).
    #[cfg(feature = "http")]
    pub fn venue_client(&mut self, group: &LimitGroup) -> Result<VenueClient, PluginError> {
        let http = self.inner.shared_http_client()?;
        Ok(VenueClient::new(http, group.clone()))
    }
}

impl ActivationContext {
    /// The lazily-built, cached [`reqwest::Client`] every HTTP-capable
    /// method on this context shares.
    #[cfg(feature = "http")]
    fn shared_http_client(&mut self) -> Result<reqwest::Client, PluginError> {
        if let Some(client) = &self.http_client {
            return Ok(client.clone());
        }
        let client = reqwest::Client::builder()
            .timeout(HTTP_REQUEST_TIMEOUT)
            .connect_timeout(HTTP_CONNECT_TIMEOUT)
            .user_agent(concat!("senken/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(PluginError::other)?;
        self.http_client = Some(client.clone());
        Ok(client)
    }
}

/// A unit of integration the runtime can activate and deactivate.
pub trait Plugin: Send + Sync {
    /// Static facts about this plugin.
    fn manifest(&self) -> PluginManifest;

    /// Registers this plugin's capabilities. Called once, before any
    /// capability is used.
    ///
    /// Required rather than defaulted: a plugin that registers nothing does
    /// nothing, so an accidental no-op should not compile.
    ///
    /// # Errors
    /// Any [`PluginError`]; the runtime treats it as fatal to startup.
    fn activate(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        #[cfg(feature = "http")]
        if self.requires_http() {
            return self.activate_with_http(&mut HttpActivationContext { inner: context });
        }
        self.activate_without_io(context)
    }

    /// Whether this module declares the HTTP capability.
    #[must_use]
    fn requires_http(&self) -> bool {
        false
    }

    /// Activates a deterministic module with no I/O capability.
    ///
    /// # Errors
    /// Any [`PluginError`]; the runtime treats it as fatal to startup.
    fn activate_without_io(&self, _context: &mut ActivationContext) -> Result<(), PluginError> {
        Err(PluginError::msg("plugin has no activation implementation"))
    }

    /// Activates a module that declared HTTP access.
    ///
    /// # Errors
    /// Any [`PluginError`]; the runtime treats it as fatal to startup.
    #[cfg(feature = "http")]
    fn activate_with_http(
        &self,
        _context: &mut HttpActivationContext<'_>,
    ) -> Result<(), PluginError> {
        Err(PluginError::msg(
            "plugin declares HTTP but has no HTTP activation implementation",
        ))
    }

    /// Releases anything held since activation. Called once, in reverse
    /// activation order, during runtime shutdown. The default does nothing.
    ///
    /// # Errors
    /// Any [`PluginError`]; the runtime logs it and keeps shutting down.
    fn deactivate(&self) -> Result<(), PluginError> {
        Ok(())
    }
}

impl fmt::Debug for dyn Plugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Plugin")
            .field("manifest", &self.manifest())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "http")]
    use super::HttpActivationContext;
    use super::{
        ActivationContext, Arc, BarSource, BookSource, FeedSource, MarketDataSource, PluginManifest,
    };
    use crate::error::PluginPermissionRegistrationError;
    use senken_acl::{PluginNamespace, PluginPermissionName};
    use senken_core::TimeRange;
    use senken_marketdata::{Instrument, SourceError, SourceSymbol};
    use senken_series::{Bar, BarSpec};
    #[cfg(feature = "http")]
    use std::time::Duration;
    #[cfg(feature = "http")]
    use wiremock::matchers::method;
    #[cfg(feature = "http")]
    use wiremock::{Mock, MockServer, ResponseTemplate};

    struct Stub;

    #[async_trait::async_trait]
    impl MarketDataSource for Stub {
        fn id(&self) -> &'static str {
            "stub"
        }

        fn name(&self) -> &'static str {
            "Stub"
        }

        async fn instruments(&self) -> Result<Vec<Instrument>, SourceError> {
            Ok(Vec::new())
        }
    }

    struct StubBars;

    #[async_trait::async_trait]
    impl BarSource for StubBars {
        fn source_id(&self) -> &'static str {
            "stub"
        }

        fn supported(&self) -> &[BarSpec] {
            &[]
        }

        fn max_rows(&self) -> usize {
            0
        }

        async fn bars(
            &self,
            _symbol: &SourceSymbol,
            _spec: BarSpec,
            _range: TimeRange,
        ) -> Result<Vec<Bar>, SourceError> {
            Ok(Vec::new())
        }
    }

    struct StubBook;

    #[async_trait::async_trait]
    impl BookSource for StubBook {
        fn source_id(&self) -> &'static str {
            "stub"
        }

        async fn book_snapshot(
            &self,
            _symbol: &SourceSymbol,
            _depth: usize,
        ) -> Result<senken_marketdata::book::BookSnapshot, SourceError> {
            Err(SourceError::rejected("stub"))
        }
    }

    struct StubFeed {
        source_ids: Vec<String>,
    }

    impl FeedSource for StubFeed {
        fn source_ids(&self) -> &[String] {
            &self.source_ids
        }

        fn serves_quotes(&self) -> bool {
            false
        }

        fn protocol(
            &self,
            _symbols: Arc<dyn senken_subscription::SymbolMap>,
        ) -> Arc<dyn senken_subscription::VenueProtocol> {
            unreachable!("no test here builds a live protocol")
        }
    }

    fn stub_feed() -> Arc<StubFeed> {
        Arc::new(StubFeed {
            source_ids: vec!["stub-spot".to_owned(), "stub-perp".to_owned()],
        })
    }

    #[test]
    fn taking_feed_sources_drains_the_context() {
        let mut context = ActivationContext::new();
        context.register_feed_source(stub_feed());

        assert_eq!(context.take_feed_sources().len(), 1);
        assert!(
            context.take_feed_sources().is_empty(),
            "a drained context must not hand the same source to the next plugin"
        );
    }

    #[test]
    fn one_feed_can_serve_several_of_a_venues_markets() {
        // A venue's physical stream is rarely split the way its markets
        // are: one socket carries spot, swap and futures. Registering one
        // feed per market would open a connection each, for the same wire.
        let feed = stub_feed();
        assert_eq!(feed.source_ids(), ["stub-spot", "stub-perp"]);
    }

    #[test]
    fn a_plugin_registering_no_feed_declares_no_live_stream() {
        let mut context = ActivationContext::new();
        context.register_marketdata_source(Arc::new(Stub));
        context.register_bar_source(Arc::new(StubBars));

        assert!(context.take_feed_sources().is_empty());
    }

    #[test]
    fn taking_book_sources_drains_the_context() {
        let mut context = ActivationContext::new();
        context.register_book_source(Arc::new(StubBook));

        assert_eq!(context.take_book_sources().len(), 1);
        assert!(
            context.take_book_sources().is_empty(),
            "a drained context must not hand the same source to the next plugin"
        );
    }

    #[test]
    fn a_book_source_states_which_source_it_serves() {
        // The runtime keys registrations by this, so a plugin that returned
        // something other than one of its own source ids would have its
        // depth filed under a venue it does not serve — or under nothing.
        let source = StubBook;
        assert_eq!(BookSource::source_id(&source), "stub");
    }

    #[test]
    fn a_plugin_registering_no_book_source_declares_no_depth() {
        // Registration *is* the capability declaration. A venue with no
        // depth must produce an empty list, never an entry serving empty
        // books — whatever reports capabilities reads this absence.
        let mut context = ActivationContext::new();
        context.register_marketdata_source(Arc::new(Stub));
        context.register_bar_source(Arc::new(StubBars));

        assert!(context.take_book_sources().is_empty());
    }

    #[test]
    fn the_four_capabilities_drain_independently() {
        // One plugin registering several must not have taking one of them
        // disturb the others — the runtime drains them separately, per
        // plugin, and a shared buffer would leak a venue's source into the
        // next plugin's registrations.
        let mut context = ActivationContext::new();
        context.register_marketdata_source(Arc::new(Stub));
        context.register_bar_source(Arc::new(StubBars));
        context.register_book_source(Arc::new(StubBook));

        assert_eq!(context.take_book_sources().len(), 1);
        assert_eq!(context.take_bar_sources().len(), 1);
        assert_eq!(context.take_marketdata_sources().len(), 1);
        assert!(context.take_feed_sources().is_empty());
    }

    #[test]
    fn taking_bar_sources_drains_the_context() {
        let mut context = ActivationContext::new();
        context.register_bar_source(Arc::new(StubBars));

        assert_eq!(context.take_bar_sources().len(), 1);
        assert!(
            context.take_bar_sources().is_empty(),
            "a drained context must not hand the same source to the next plugin"
        );
    }

    #[test]
    fn taking_sources_drains_the_context() {
        let mut context = ActivationContext::new();
        context.register_marketdata_source(Arc::new(Stub));

        assert_eq!(context.take_marketdata_sources().len(), 1);
        assert!(
            context.take_marketdata_sources().is_empty(),
            "a drained context must not hand the same source to the next plugin"
        );
    }

    #[cfg(feature = "http")]
    #[test]
    fn the_shared_http_client_is_built_once_and_cached() {
        // Exercises `shared_http_client` directly rather than the deprecated
        // `http_client` wrapper, so this crate's own test suite does not
        // trip `-D warnings` on its own deprecation notice.
        let mut context = ActivationContext::new();
        assert!(context.http_client.is_none(), "nothing is built eagerly");

        let first = context.shared_http_client().unwrap();
        assert!(
            context.http_client.is_some(),
            "the client must be cached so the next plugin shares this pool"
        );

        let second = context.shared_http_client().unwrap();
        drop((first, second));
    }

    #[cfg(feature = "http")]
    #[test]
    fn a_limit_group_configures_windows_and_concurrency_by_chaining() {
        let mut context = ActivationContext::new();
        let mut http = HttpActivationContext {
            inner: &mut context,
        };
        let group = http
            .limit_group("test-venue")
            .per_window(std::time::Duration::from_secs(1), 10)
            .max_concurrent(2);
        assert_eq!(group.name(), "test-venue");
    }

    #[cfg(feature = "http")]
    #[test]
    fn a_group_a_plugin_never_configures_still_has_a_ceiling() {
        // The regression this catches, which already happened once: a
        // proactive budget lived at one call site, that call site was
        // deleted in a refactor, and every venue's traffic — including
        // order-book depth, which *polls* — was left with no ceiling at
        // all. Nothing failed, nothing warned; it would have surfaced as a
        // ban.
        let mut context = ActivationContext::new();
        let mut http = HttpActivationContext {
            inner: &mut context,
        };
        let group = http.limit_group("silent-venue");

        let windows = group.windows();
        assert!(
            !windows.is_empty(),
            "a plugin that configures nothing must still be bounded"
        );
        assert!(
            windows
                .iter()
                .all(|&(window, budget)| { window > Duration::ZERO && budget > 0 })
        );
    }

    #[cfg(feature = "http")]
    #[test]
    fn a_plugins_own_window_composes_with_the_default_rather_than_replacing_it() {
        // `per_window` adds; a request must fit inside *every* window. A
        // plugin narrowing its own budget must not accidentally widen the
        // one it was given.
        let mut context = ActivationContext::new();
        let mut http = HttpActivationContext {
            inner: &mut context,
        };
        let group = http
            .limit_group("careful-venue")
            .per_window(Duration::from_mins(1), 5);

        assert_eq!(group.windows().len(), 2);
    }

    #[cfg(feature = "http")]
    #[test]
    fn a_venue_client_is_built_from_the_shared_http_client_and_a_group() {
        let mut context = ActivationContext::new();
        let mut http = HttpActivationContext {
            inner: &mut context,
        };
        let group = http.limit_group("test-venue");
        let client = http.venue_client(&group).unwrap();
        drop(client);
        assert!(
            context.http_client.is_some(),
            "venue_client must reuse the same cached http client"
        );
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn two_limit_group_calls_with_the_same_name_share_one_budget() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let mut context = ActivationContext::new();
        let mut http = HttpActivationContext {
            inner: &mut context,
        };
        let instrument_group = http
            .limit_group("shared-venue")
            .per_window(Duration::from_mins(1), 2);
        let bar_group = http.limit_group("shared-venue");
        let instrument_client = http.venue_client(&instrument_group).unwrap();
        let bar_client = http.venue_client(&bar_group).unwrap();

        instrument_client.get(&server.uri(), 1).await.unwrap();
        bar_client.get(&server.uri(), 1).await.unwrap();
        assert!(
            tokio::time::timeout(
                Duration::from_millis(50),
                instrument_client.get(&server.uri(), 1),
            )
            .await
            .is_err(),
            "limit groups with the same name must share one budget"
        );
    }

    #[cfg(feature = "http")]
    #[tokio::test]
    async fn limit_group_calls_with_different_names_stay_independent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
            .mount(&server)
            .await;

        let mut context = ActivationContext::new();
        let mut http = HttpActivationContext {
            inner: &mut context,
        };
        let a = http
            .limit_group("venue-a")
            .per_window(Duration::from_mins(1), 1);
        let b = http
            .limit_group("venue-b")
            .per_window(Duration::from_mins(1), 1);
        let client_a = http.venue_client(&a).unwrap();
        let client_b = http.venue_client(&b).unwrap();

        client_a.get(&server.uri(), 1).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(50), client_b.get(&server.uri(), 1))
                .await
                .is_ok(),
            "differently named venues must not share a budget"
        );
    }

    fn manifest(id: &str, permissions: Vec<PluginPermissionName>) -> PluginManifest {
        PluginManifest {
            id: id.to_owned(),
            name: id.to_owned(),
            version: "0".to_owned(),
            description: String::new(),
            permissions,
        }
    }

    #[test]
    fn a_manifest_with_no_permissions_validates_cleanly() {
        manifest("mychart", Vec::new())
            .validate_permissions()
            .unwrap();
    }

    #[test]
    fn a_manifest_declaring_a_permission_inside_its_own_namespace_validates() {
        let view = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
        manifest("mychart", vec![view])
            .validate_permissions()
            .unwrap();
    }

    #[test]
    fn a_manifest_declaring_a_permission_outside_its_own_namespace_fails_to_validate() {
        // The scenario B9 calls out by name: a manifest cannot delegate
        // authority over anything but its own subtree, even in its own
        // static, build-time declaration.
        let foreign = PluginPermissionName::parse("senken.users:manage").unwrap();
        assert!(
            manifest("mychart", vec![foreign])
                .validate_permissions()
                .is_err()
        );
    }

    #[test]
    fn registering_a_permission_before_any_namespace_is_bound_is_rejected() {
        let mut context = ActivationContext::new();
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();

        let err = context.register_plugin_permission(name).unwrap_err();

        assert!(matches!(
            err,
            PluginPermissionRegistrationError::NoNamespaceBound
        ));
    }

    #[test]
    fn registering_a_permission_inside_the_bound_namespace_succeeds() {
        let mut context = ActivationContext::new();
        context.bind_permission_namespace(PluginNamespace::new("mychart").unwrap());
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();

        context.register_plugin_permission(name.clone()).unwrap();

        assert_eq!(context.take_plugin_permissions(), vec![name]);
    }

    #[test]
    fn registering_a_permission_outside_the_bound_namespace_is_rejected() {
        // A plugin bound to `mychart` cannot register into `senken`'s
        // namespace (or any other plugin's) — the manifest delegates
        // authority over its own subtree only, like a DNS zone.
        let mut context = ActivationContext::new();
        context.bind_permission_namespace(PluginNamespace::new("mychart").unwrap());
        let foreign = PluginPermissionName::parse("senken.users:manage").unwrap();

        let err = context.register_plugin_permission(foreign).unwrap_err();

        assert!(matches!(
            err,
            PluginPermissionRegistrationError::InvalidPermission(_)
        ));
        assert!(
            context.take_plugin_permissions().is_empty(),
            "a rejected registration must not leave a trace behind"
        );
    }

    #[test]
    fn taking_plugin_permissions_drains_the_context_and_unbinds_the_namespace() {
        let mut context = ActivationContext::new();
        context.bind_permission_namespace(PluginNamespace::new("mychart").unwrap());
        let name = PluginPermissionName::parse("mychart.dashboard:view").unwrap();
        context.register_plugin_permission(name).unwrap();

        assert_eq!(context.take_plugin_permissions().len(), 1);
        assert!(
            context.take_plugin_permissions().is_empty(),
            "a drained context must not hand the same registration to the next plugin"
        );

        // The namespace was unbound by the first `take`, so the next
        // plugin's registration must fail until the runtime binds its own.
        let name = PluginPermissionName::parse("mychart.dashboard:edit").unwrap();
        let err = context.register_plugin_permission(name).unwrap_err();
        assert!(matches!(
            err,
            PluginPermissionRegistrationError::NoNamespaceBound
        ));
    }
}
