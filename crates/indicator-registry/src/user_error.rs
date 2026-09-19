//! Everything that can go wrong in [`crate::UserIndicatorStore`].

/// Why a user-indicator operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum UserIndicatorError {
    /// The underlying SQLite call failed.
    #[error("sqlite operation failed")]
    Database(#[from] rusqlite::Error),

    /// The permission check itself — reused so a caller sees the exact
    /// same [`PasswordNotSet`](senken_identity::IdentityError::PasswordNotSet)/
    /// [`Forbidden`](senken_identity::IdentityError::Forbidden) every other
    /// guarded query in this workspace produces for the same reasons.
    #[error(transparent)]
    Identity(#[from] senken_identity::IdentityError),

    /// No indicator exists at the given id, **or** it exists but belongs
    /// to a different account. The two are reported identically —
    /// [`crate::UserIndicatorStore::get`] treats a user's own source the
    /// way `senken_trade::TradeAccountStore::settings_for` treats
    /// credentials: whether someone else's indicator by this id exists at
    /// all is not this caller's to learn, so the response carries no
    /// distinction between "does not exist" and "exists, not yours".
    #[error("no such indicator")]
    NotFound,

    /// The requested title/slug is already used by another indicator this
    /// account owns.
    #[error("you already have an indicator named `{0}`")]
    DuplicateSlug(String),
}
