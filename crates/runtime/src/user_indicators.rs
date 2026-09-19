//! [`UserIndicators`]: per-account catalogs of indicators an account
//! compiled themselves from their own Rust source, alongside
//! [`crate::plugin_host::DynamicIndicators`]'s shared, admin-managed
//! catalog of uploaded plugins.
//!
//! # Why a separate type, not one more origin in the shared catalog
//!
//! [`crate::plugin_host::DynamicIndicators`] is one catalog, shared by
//! every account: an uploaded plugin is visible to whoever can reach `GET
//! /api/indicators`. An account's own compiled indicator must not be —
//! `senken-indicator-registry::UserIndicatorStore` already scopes who may
//! read one's *source*, and a catalog that only isolates by name (rather
//! than by account) would let one user's private indicator show up, and
//! be computable, in anyone else's chart the moment two accounts picked
//! the same slug. So each account gets its own
//! [`crate::plugin_host::DynamicIndicators`] instance — its own entry
//! table — never the shared one.
//!
//! All of them share one [`senken_plugin_host::PluginHost`] (one `wasmtime::Engine`), the same
//! way every built-in and uploaded plugin does — an `Engine` is the
//! expensive part to build, and nothing about isolating catalogs by
//! account needs a second one.

use std::collections::HashMap;
use std::sync::{PoisonError, RwLock};

use senken_identity::UserId;
use senken_plugin_host::PluginHost;

use crate::plugin_host::{
    DynamicIndicatorError, DynamicIndicatorInfo, DynamicIndicatorInstance, DynamicIndicators,
    PluginOrigin,
};

/// Per-account catalogs of self-compiled indicators, all sharing one
/// [`PluginHost`].
pub struct UserIndicators {
    host: PluginHost,
    per_owner: RwLock<HashMap<UserId, DynamicIndicators>>,
}

impl std::fmt::Debug for UserIndicators {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let owners = self
            .per_owner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .len();
        f.debug_struct("UserIndicators")
            .field("owners", &owners)
            .finish_non_exhaustive()
    }
}

impl UserIndicators {
    /// Builds an empty registry sharing `host` across every account's own
    /// catalog, created lazily on first use.
    #[must_use]
    pub fn new(host: PluginHost) -> Self {
        Self {
            host,
            per_owner: RwLock::new(HashMap::new()),
        }
    }

    /// `owner`'s own catalog, creating an empty one the first time this
    /// account is seen. Cheap to call repeatedly:
    /// [`DynamicIndicators`] clones share their table, so this never
    /// duplicates a loaded component.
    fn catalog_for(&self, owner: UserId) -> DynamicIndicators {
        if let Some(existing) = self
            .per_owner
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&owner)
        {
            return existing.clone();
        }
        self.per_owner
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(owner)
            .or_insert_with(|| DynamicIndicators::with_host(self.host.clone()))
            .clone()
    }

    /// Compiles and registers `wasm` under `name` (the catalog name —
    /// `my/<slug>`, assigned once by the caller, never the component's own
    /// `descriptor().id`) in `owner`'s own catalog, replacing whatever was
    /// already registered under that name.
    ///
    /// # Errors
    /// [`DynamicIndicatorError::Host`] if the component fails to load.
    pub fn load(
        &self,
        owner: UserId,
        name: &str,
        wasm: &[u8],
    ) -> Result<(), DynamicIndicatorError> {
        self.catalog_for(owner)
            .register_named(name, wasm, PluginOrigin::User)?;
        Ok(())
    }

    /// Removes `name` from `owner`'s own catalog. A no-op if nothing was
    /// registered under it.
    pub fn unload(&self, owner: UserId, name: &str) {
        let _removed = self.catalog_for(owner).unregister(name);
    }

    /// `owner`'s own catalog entries, currently enabled — merged into `GET
    /// /api/indicators` for that account only, the way
    /// [`crate::plugin_host::DynamicIndicators::catalog`] is merged in for
    /// everyone.
    #[must_use]
    pub fn catalog(&self, owner: UserId) -> Vec<DynamicIndicatorInfo> {
        self.catalog_for(owner).catalog()
    }

    /// Spawns a fresh instance of `owner`'s own indicator `name`, the same
    /// way [`crate::plugin_host::DynamicIndicators::spawn`] does for the
    /// shared catalog.
    ///
    /// # Errors
    /// [`DynamicIndicatorError::UnknownPlugin`] if `owner` has nothing
    /// registered under `name` (including if `owner` has never compiled
    /// anything at all); otherwise as
    /// [`crate::plugin_host::DynamicIndicators::spawn`].
    pub fn spawn(
        &self,
        owner: UserId,
        name: &str,
        params_json: &str,
    ) -> Result<DynamicIndicatorInstance, DynamicIndicatorError> {
        self.catalog_for(owner).spawn(name, params_json)
    }
}

#[cfg(test)]
mod tests {
    use senken_identity::UserId;
    use senken_plugin_host::{PluginHost, PluginLimits};

    use super::UserIndicators;

    /// No real component load here — see
    /// `crates/runtime/tests/user_indicators.rs` for the version of this
    /// property proven against a genuinely compiled `.wasm`. This unit
    /// test only needs to prove the *empty*-catalog half without paying
    /// for a `cargo build`: an owner nothing was ever loaded for gets an
    /// empty catalog and an `UnknownPlugin` error, never another owner's.
    #[test]
    fn an_owner_with_nothing_loaded_has_an_empty_catalog_and_cannot_spawn() {
        let host = PluginHost::new(PluginLimits::default()).unwrap();
        let registry = UserIndicators::new(host);
        let owner = UserId::new();

        assert!(registry.catalog(owner).is_empty());
        assert!(registry.spawn(owner, "my/nothing", "{}").is_err());
    }
}
