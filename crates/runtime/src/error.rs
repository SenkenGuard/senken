use senken_identity::IdentityError;
use senken_marketdata::MarketDataError;
use senken_plugin::PluginError;
use senken_storage::StorageError;

/// Why the runtime could not start or stop.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RuntimeError {
    /// The data directory could not be prepared.
    #[error("storage initialisation failed")]
    StorageInit {
        /// The storage failure.
        #[source]
        source: StorageError,
    },

    /// A plugin refused to activate. Startup stops at the first failure.
    #[error("plugin `{plugin}` failed to activate")]
    PluginActivation {
        /// The plugin's manifest id.
        plugin: String,
        /// What the plugin reported.
        #[source]
        source: PluginError,
    },

    /// A plugin failed to deactivate. Shutdown continues past it.
    #[error("plugin `{plugin}` failed to deactivate")]
    PluginDeactivation {
        /// The plugin's manifest id.
        plugin: String,
        /// What the plugin reported.
        #[source]
        source: PluginError,
    },

    /// Two plugins registered the same plugin id.
    #[error("plugin id `{0}` is registered twice")]
    DuplicatePlugin(String),

    /// A plugin contributed a source the market data registry rejected.
    #[error("plugin `{plugin}` contributed an unusable market data source")]
    SourceRegistration {
        /// The plugin's manifest id.
        plugin: String,
        /// Why the registry refused it.
        #[source]
        source: MarketDataError,
    },

    /// A plugin contributed a trade adapter the engine rejected — a
    /// duplicate id, or one that is not a valid slug — or one this
    /// activation had nowhere to register it at all: `TradeEngine` is
    /// shared behind an `Arc` once the server is running, so a plugin
    /// activated after startup that registers an adapter it never
    /// declared in its own manifest hits this too (see
    /// `drain_registrations`'s own docs).
    #[error("plugin `{plugin}` contributed an unusable trade adapter")]
    TradeAdapterRegistration {
        /// The plugin's manifest id.
        plugin: String,
        /// Why the engine refused it.
        #[source]
        source: senken_trade::TradeError,
    },

    /// The bar-series store directory could not be prepared.
    #[error("series store initialisation failed")]
    SeriesStoreInit {
        /// The store failure.
        #[source]
        source: senken_store::StoreError,
    },

    /// The dynamic-indicator plugin host could not be built.
    #[error("dynamic indicator host initialisation failed")]
    DynamicIndicatorHostInit {
        /// The underlying failure.
        #[source]
        source: crate::plugin_host::DynamicIndicatorError,
    },

    /// The widget UI package store's directory could not be prepared.
    #[error("widget plugin store initialisation failed")]
    WidgetPluginStoreInit {
        /// The underlying failure.
        #[source]
        source: senken_plugin::widget_package::WidgetPackageError,
    },

    /// The default plugin enable/disable state could not be seeded on a
    /// fresh `plugin_state` table.
    #[error("could not seed the default plugin enable/disable state")]
    PluginStateInit {
        /// The underlying failure.
        #[source]
        source: IdentityError,
    },

    /// The dynamic-venue plugin host could not be built. A single venue
    /// package failing to *load* is not this — that is recorded as a
    /// `Failed` catalog entry (see `crate::plugin_host::DynamicVenues`) and
    /// never aborts startup; this is the shared host itself failing to
    /// come up at all.
    #[error("dynamic venue host initialisation failed")]
    DynamicVenueHostInit {
        /// The underlying failure.
        #[source]
        source: crate::plugin_host::DynamicVenueError,
    },
}
