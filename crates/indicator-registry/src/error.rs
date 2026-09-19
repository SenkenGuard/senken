//! Everything that can go wrong in the indicator registry.

/// Why a registry operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum RegistryError {
    /// The underlying SQLite call failed.
    #[error("sqlite operation failed")]
    Database(#[from] rusqlite::Error),

    /// The permission check itself — `senken_identity::AuthenticatedUser::authorize`'s
    /// own error, reused rather than re-declared so a caller sees the exact
    /// same [`PasswordNotSet`](senken_identity::IdentityError::PasswordNotSet)/
    /// [`Forbidden`](senken_identity::IdentityError::Forbidden) every other
    /// guarded query in this workspace produces for the same reasons.
    #[error(transparent)]
    Identity(#[from] senken_identity::IdentityError),

    /// A publish request named a namespace other than the caller's own
    /// account. This is the check that closes author impersonation (see
    /// this crate's module docs) — never widened, regardless of what a
    /// caller's grants say, because a namespace is an identity fact, not a
    /// permission level.
    #[error("you may only publish into your own namespace")]
    ForeignNamespace,

    /// No indicator exists at the given `(namespace, name)`, or for the
    /// given id.
    #[error("no such indicator in the registry")]
    NotFound,

    /// The submitted name is not a legal indicator name (empty, or
    /// containing a `/` — which would make a qualified name ambiguous to
    /// parse back apart).
    #[error("`{0}` is not a valid indicator name")]
    InvalidName(String),

    /// The published indicator's recorded language version is newer than
    /// what this host currently compiles — see this crate's module docs
    /// for why this can happen even though publishing itself always
    /// records the *publishing* host's own version.
    #[error(
        "this indicator needs language version {required}, but this host only understands up to {host}"
    )]
    LanguageVersionTooNew {
        /// The version recorded on the published indicator.
        required: String,
        /// The version this host currently compiles.
        host: String,
    },

    /// `publish`'s own handle gate found no row in `registry_handles` for
    /// the publishing account. Nothing in this build can ever populate
    /// that table any more (see this crate's module docs), so this is now
    /// the answer every publish attempt gets.
    #[error("choose a registry handle before publishing")]
    HandleNotSet,
}
