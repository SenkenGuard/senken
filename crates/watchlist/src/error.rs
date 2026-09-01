//! Everything that can go wrong in the watchlist store.

/// Why a watchlist-store operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum WatchlistError {
    /// The underlying SQLite call failed — including a constraint violation
    /// partway through a multi-row reorder, which this variant also
    /// carries: the transaction is rolled back before this ever reaches the
    /// caller.
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

    /// No watchlist group exists for the given id.
    #[error("no watchlist group found for that id")]
    GroupNotFound,

    /// No watchlist member exists for the given id.
    #[error("no watchlist member found for that id")]
    MemberNotFound,

    /// A `watchlist_members.instrument` column held text that no longer
    /// parses as a `senken_marketdata::InstrumentId` — the database was
    /// written by an incompatible version of this crate, or edited by
    /// hand.
    #[error("stored instrument id is corrupt: {0}")]
    CorruptInstrument(String),
}
