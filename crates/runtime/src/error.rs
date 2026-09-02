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
}
