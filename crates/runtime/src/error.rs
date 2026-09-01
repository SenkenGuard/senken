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
    /// duplicate id, or one that is not a valid slug.
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
}
