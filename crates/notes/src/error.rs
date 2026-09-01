//! Everything that can go wrong in the notes store.

/// Why a notes-store operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum NoteError {
    /// The underlying SQLite call failed.
    #[error("sqlite operation failed")]
    Database(#[from] rusqlite::Error),

    /// The permission check itself — `senken_identity::AuthenticatedUser::authorize`'s
    /// own error, reused rather than re-declared so a caller sees the exact
    /// same [`PasswordNotSet`](senken_identity::IdentityError::PasswordNotSet)/
    /// [`Forbidden`](senken_identity::IdentityError::Forbidden) this crate's
    /// own guarded queries and `senken-identity`'s produce for the same
    /// reasons.
    #[error(transparent)]
    Identity(#[from] senken_identity::IdentityError),

    /// No note exists for the given id.
    #[error("no note found for that id")]
    NoteNotFound,
}
