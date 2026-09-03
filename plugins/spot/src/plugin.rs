//! Registering the spot adapter with a runtime.

use std::sync::Arc;

use senken_plugin::{ActivationContext, Plugin, PluginError, PluginManifest};
use senken_sim_core::SimAdapter;
use senken_storage::Storage;
use senken_trade::TradeAdapter;

use crate::venue::{ADAPTER_ID, SpotVenue};

/// The plugin that registers the spot adapter.
#[derive(Debug)]
pub struct SpotPlugin {
    adapter: Arc<SimAdapter<SpotVenue>>,
}

impl SpotPlugin {
    /// Builds the plugin over `storage`, where its balances live.
    #[must_use]
    pub fn new(storage: Storage) -> Self {
        Self {
            adapter: Arc::new(SimAdapter::new(SpotVenue, storage)),
        }
    }
}

impl Plugin for SpotPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ADAPTER_ID.to_owned(),
            name: "Spot exchange".to_owned(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            description: "A simulated spot account: asset balances, no leverage, no short"
                .to_owned(),
            permissions: Vec::new(),
        }
    }

    fn activate_without_io(&self, context: &mut ActivationContext) -> Result<(), PluginError> {
        context.register_trade_adapter(Arc::clone(&self.adapter) as Arc<dyn TradeAdapter>);
        Ok(())
    }
}
