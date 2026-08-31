use std::error::Error;

use senken_acl::PluginPermissionError;

/// A type-erased error a plugin can wrap.
pub type BoxError = Box<dyn Error + Send + Sync>;

/// Why a plugin could not activate or deactivate.
///
/// The runtime attaches the plugin id when it reports the failure, so a
/// plugin only needs to say what went wrong, not who it is.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginError {
    /// A plain explanation with no underlying cause.
    #[error("{0}")]
    Message(String),

    /// An underlying failure, preserved for the error chain.
    #[error(transparent)]
    Other(BoxError),
}

impl PluginError {
    /// A failure explained in words.
    #[must_use]
    pub fn msg(message: impl Into<String>) -> Self {
        Self::Message(message.into())
    }

    /// A failure caused by another error.
    pub fn other(source: impl Into<BoxError>) -> Self {
        Self::Other(source.into())
    }
}

/// Why [`crate::ActivationContext::register_plugin_permission`] refused a
/// runtime permission registration.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PluginPermissionRegistrationError {
    /// No namespace is bound to this activation yet. The runtime binds one,
    /// from the manifest it already trusts, immediately before calling
    /// [`crate::Plugin::activate`] — this only fires if a permission is
    /// registered outside that call, which nothing in this crate does.
    #[error("no permission namespace is bound to this activation")]
    NoNamespaceBound,

    /// The permission does not belong to the namespace bound to this
    /// activation — the manifest delegates authority over its own subtree
    /// only, like a DNS zone, and this is the runtime-registration half of
    /// that rule (the build-time half is
    /// [`crate::PluginManifest::validate_permissions`]).
    #[error(transparent)]
    InvalidPermission(#[from] PluginPermissionError),
}
