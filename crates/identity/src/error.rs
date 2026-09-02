//! Everything that can go wrong in the identity store.

/// Why an identity-store operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum IdentityError {
    /// The underlying SQLite call failed.
    #[error("sqlite operation failed")]
    Database(#[from] rusqlite::Error),

    /// The accounts directory could not be created.
    #[error("could not prepare the accounts directory")]
    Io(#[from] std::io::Error),

    /// Argon2 hashing failed for a reason other than "wrong password"
    /// (which is not an error at all — see
    /// [`IdentityStore::login`](crate::IdentityStore::login), which treats
    /// a failed verification as [`InvalidCredentials`](Self::InvalidCredentials),
    /// never as this variant).
    #[error("password hashing failed")]
    Hashing(#[from] argon2::password_hash::Error),

    /// The database file's schema `user_version` is neither `0` (fresh) nor
    /// the version this crate knows how to use. There is no migration
    /// crate, so a mismatch is reported rather than guessed at.
    #[error("accounts database has schema version {found}, expected {expected}")]
    SchemaVersionMismatch {
        /// Version recorded in the database.
        found: i32,
        /// Version this build of the crate expects.
        expected: i32,
    },

    /// A password shorter than the minimum was supplied (a length floor is the only composition rule).
    #[error("password must be at least {minimum} characters")]
    PasswordTooShort {
        /// The minimum required length.
        minimum: usize,
    },

    /// `email` is already registered to another account.
    #[error("email `{0}` is already registered")]
    EmailTaken(String),

    /// No user exists for the email supplied to an operation that, unlike
    /// [`IdentityStore::login`](crate::IdentityStore::login), has no reason
    /// to hide that fact (e.g. an admin looking up a user to edit).
    #[error("no user found for that email")]
    UserNotFound,

    /// Login failed. Deliberately the *same* error for "no such account"
    /// and "wrong password" — do not add a variant that
    /// distinguishes them, that is the enumeration hole this type exists to
    /// close.
    #[error("invalid email or password")]
    InvalidCredentials,

    /// The account's password is unset: every operation
    /// except setting the password is refused while this fence is up.
    #[error("this account has not set a password yet")]
    PasswordNotSet,

    /// `senken_acl::decide` denied the action, or returned a [`Scope`] this
    /// crate does not know how to turn into a query — the
    /// latter guards against `Scope` growing a new variant (it is
    /// `#[non_exhaustive]`) that this crate has not been taught to handle
    /// yet; failing closed is the only safe default for an unknown scope.
    ///
    /// [`Scope`]: senken_acl::Scope
    #[error("actor is not permitted to perform this action")]
    Forbidden,

    /// A grant, or a request to grant one, named a plugin permission no
    /// plugin has ever registered — there is no
    /// `plugin_permissions` row for it at all to grant.
    #[error("plugin permission `{0}` has never been registered")]
    PluginPermissionNotFound(String),

    /// A grant, or a request to grant one, named a plugin permission that
    /// was registered once but whose owning plugin has since stopped
    /// declaring it (the orphan state). An orphan stays attached
    /// to whatever already holds it — it is not silently dropped — but it
    /// is not newly grantable while orphaned.
    #[error("plugin permission `{0}` is orphaned and cannot be newly granted")]
    PluginPermissionOrphaned(String),

    /// A `role_grants` or `user_grants` row named an `action`/`resource`
    /// this build of `senken-acl` does not know, or the column held text
    /// that is not one of the values this crate ever writes. Either the
    /// database was written by a different version of this crate or the
    /// row was edited by hand; either way, guessing at the intended grant
    /// is not safe.
    #[error("accounts database contains an unrecognised grant: {0}")]
    CorruptGrant(String),

    /// `users.display_zone` held a string the bundled time zone database no
    /// longer recognises. This crate only ever writes a value that already
    /// passed `senken_core::IanaZone::new`, so reaching this means the row
    /// was written by an incompatible version of this crate or edited by
    /// hand — the same "do not guess" reasoning as
    /// [`CorruptGrant`](Self::CorruptGrant).
    #[error("accounts database contains an unrecognised time zone: {0}")]
    CorruptZone(String),
}
