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

    /// The source failed to compile. Carries
    /// [`senken_indicator_lang::CompileError`]'s own line/column message
    /// verbatim — the same message a trader would see authoring this
    /// indicator directly, not a registry-specific rewording.
    #[error(transparent)]
    InvalidSource(#[from] senken_indicator_lang::CompileError),

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

    /// The submitted handle is not a legal registry handle — see
    /// [`crate::Handle::new`] for the exact rule.
    #[error("`{0}` is not a valid registry handle")]
    InvalidHandle(String),

    /// Another account already holds this handle. A handle is a pointer at
    /// exactly one account (see this crate's `handle` module docs), so
    /// claiming one already claimed is refused rather than moved.
    #[error("handle `{0}` is already taken")]
    HandleTaken(String),

    /// No account has claimed the handle a caller addressed by.
    #[error("no account has claimed the handle `{0}`")]
    HandleNotFound(String),

    /// The publishing account has not chosen a registry handle yet.
    /// Publishing is refused rather than accepted under an address nobody
    /// else can type — see this crate's module docs for why a registry
    /// entry addressable only by raw account id defeats the point of a
    /// registry meant for people to search and install from.
    #[error("choose a registry handle before publishing")]
    HandleNotSet,
}
