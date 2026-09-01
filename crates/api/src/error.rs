//! Everything that can go wrong starting or running the server.

/// Errors from [`crate::serve`].
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ApiError {
    /// The requested host/port could not be bound.
    #[error("failed to bind {addr}")]
    Bind {
        /// The address that was requested.
        addr: std::net::SocketAddr,
        /// The underlying OS error.
        #[source]
        source: std::io::Error,
    },

    /// The server's local address could not be read back after binding.
    #[error("failed to read the bound local address")]
    LocalAddr(#[source] std::io::Error),

    /// The server task exited with an I/O error rather than a clean
    /// graceful shutdown.
    #[error("the server exited with an error")]
    Serve(#[source] std::io::Error),

    /// The server task panicked or was cancelled before it could shut down.
    #[error("server task did not shut down cleanly")]
    Join(#[source] tokio::task::JoinError),
}

/// A failure while handling one HTTP request, mapped to a
/// status code and a uniform [`crate::dto::ErrorBody`] by
/// [`axum::response::IntoResponse`] — never a raw [`senken_identity::IdentityError`]
/// or [`std::error::Error`] message, which could leak internals a client
/// has no business seeing.
#[derive(Debug)]
pub(crate) enum HandlerError {
    /// `400 Bad Request`: the request itself is malformed or ineligible
    /// (e.g. a `set-password` call for an account that is not fenced, with
    /// no session to prove ownership).
    BadRequest(String),
    /// `401 Unauthorized`: no valid session. a client must
    /// clear its stored credential and route to login on this, exactly
    /// once.
    Unauthorized,
    /// `401 Unauthorized`, but for a login attempt that was answered rather
    /// than a session that ran out. Both are the same status — the request
    /// did not carry a usable identity either way — but they mean opposite
    /// things to the person reading the screen: one says "sign in again",
    /// the other says "that email and password do not match". Sharing
    /// [`Unauthorized`](Self::Unauthorized)'s body left a failed login
    /// telling the user their session had expired, when they had no session
    /// to expire.
    ///
    /// The message deliberately does not say *which* half was wrong. An
    /// account that exists and one that does not must be indistinguishable
    /// to anyone probing this endpoint, which is the same reason
    /// `IdentityStore::login` returns one error for both.
    InvalidCredentials,
    /// `403 Forbidden`: a session is valid, but the account is still behind
    /// the B4 fence, or (once senken-acl-checked endpoints exist) the
    /// actor lacks the grant. never a logout, only a message.
    Forbidden(String),
    /// `409 Conflict`: e.g. the email is already registered.
    Conflict(String),
    /// `429 Too Many Requests`: the login rate limit.
    TooManyRequests,
    /// `500 Internal Server Error`: a storage failure or similar, logged
    /// here (so the cause is not lost) and reported to the client with no
    /// detail beyond the status.
    Internal,
}

impl From<senken_identity::IdentityError> for HandlerError {
    /// Every handler's translation from the store's error type. An identical
    /// response for unknown-user and wrong-password is
    /// automatic here, not a separate case this function has to get right:
    /// `senken_identity::IdentityStore::login` already collapses both
    /// causes into the single `InvalidCredentials` variant, so both reach
    /// this same arm and produce the exact same status and body.
    fn from(error: senken_identity::IdentityError) -> Self {
        use senken_identity::IdentityError;
        match error {
            IdentityError::PasswordTooShort { minimum } => {
                Self::BadRequest(format!("password must be at least {minimum} characters"))
            }
            IdentityError::EmailTaken(email) => {
                Self::Conflict(format!("email `{email}` is already registered"))
            }
            IdentityError::UserNotFound => Self::BadRequest("no such account".to_owned()),
            IdentityError::PluginPermissionNotFound(name) => Self::BadRequest(format!(
                "plugin permission `{name}` has never been registered"
            )),
            IdentityError::PluginPermissionOrphaned(name) => Self::BadRequest(format!(
                "plugin permission `{name}` is orphaned and cannot be newly granted"
            )),
            IdentityError::InvalidCredentials => Self::InvalidCredentials,
            IdentityError::PasswordNotSet => {
                Self::Forbidden("this account has not set a password yet".to_owned())
            }
            IdentityError::Forbidden => {
                Self::Forbidden("you do not have permission to do that".to_owned())
            }
            IdentityError::Database(source) => {
                tracing::error!(%source, "identity store: database error");
                Self::Internal
            }
            IdentityError::Io(source) => {
                tracing::error!(%source, "identity store: io error");
                Self::Internal
            }
            IdentityError::Hashing(source) => {
                tracing::error!(%source, "identity store: hashing error");
                Self::Internal
            }
            IdentityError::SchemaVersionMismatch { found, expected } => {
                tracing::error!(found, expected, "identity store: schema version mismatch");
                Self::Internal
            }
            IdentityError::CorruptGrant(detail) => {
                tracing::error!(detail, "identity store: corrupt grant");
                Self::Internal
            }
            // `IdentityError` is `#[non_exhaustive]` (the // no-migration-crate note applies here too): a future variant
            // must fail closed rather than being silently accepted as a
            // 500 with no record of what happened.
            other => {
                tracing::error!(?other, "identity store: unmapped error variant");
                Self::Internal
            }
        }
    }
}

/// `workspace_handlers`' translation from `senken_chart::ChartWorkspaceStore`'s
/// error type. `ChartError::Identity` reuses this
/// crate's own `IdentityError` conversion above, so the exact same
/// `Forbidden`/`PasswordNotSet` mapping every other guarded store already
/// gets here applies to chart workspaces too — including the required "the
/// wrong user gets 403, not 401" behaviour.
impl From<senken_chart::ChartError> for HandlerError {
    fn from(error: senken_chart::ChartError) -> Self {
        use senken_chart::ChartError;
        match error {
            ChartError::Identity(source) => source.into(),
            ChartError::WorkspaceNotFound => Self::BadRequest("no such workspace".to_owned()),
            ChartError::LayoutNotFound => Self::BadRequest("no such layout".to_owned()),
            ChartError::PaneItemNotFound => Self::BadRequest("no such pane item".to_owned()),
            ChartError::PaneCountMismatch {
                preset,
                expected,
                actual,
            } => Self::BadRequest(format!(
                "layout preset {preset} requires exactly {expected} panes, got {actual}"
            )),
            ChartError::InvalidIndicatorParams(reason) => Self::BadRequest(format!(
                "indicator item parameters are not valid JSON: {reason}"
            )),
            ChartError::InvalidDrawingStyle(reason) => {
                Self::BadRequest(format!("invalid drawing style: {reason}"))
            }
            ChartError::ItemSourceMismatch => Self::BadRequest(
                "cannot change a pane item's kind through this endpoint".to_owned(),
            ),
            ChartError::InvalidPaneSettings(reason) => {
                Self::BadRequest(format!("pane settings are not valid JSON: {reason}"))
            }
            ChartError::InvalidWorkspaceSettings(reason) => Self::BadRequest(format!(
                "workspace settings must be a JSON object: {reason}"
            )),
            ChartError::Database(source) => {
                tracing::error!(%source, "chart store: database error");
                Self::Internal
            }
            // `CorruptInstrumentId`/`CorruptTimeframe`/`CorruptPreset`/
            // `CorruptPaneItem`/`CorruptDrawing`, plus any future
            // `#[non_exhaustive]` variant: a stored row this build cannot
            // even parse back is this crate's own bug or a hand-edited
            // database, never something a caller's request could have
            // caused.
            other => {
                tracing::error!(?other, "chart store: unmapped error variant");
                Self::Internal
            }
        }
    }
}

/// `alert_handlers`' translation from `senken_alerts::AlertStore`'s error
/// type, mirroring [`From<senken_chart::ChartError>`]
/// exactly — the same guarded-store shape, the same "the store's `Identity`
/// error reuses this crate's own `IdentityError` mapping" reasoning.
impl From<senken_alerts::AlertError> for HandlerError {
    fn from(error: senken_alerts::AlertError) -> Self {
        use senken_alerts::AlertError;
        match error {
            AlertError::Identity(source) => source.into(),
            AlertError::AlertNotFound => Self::BadRequest("no such alert".to_owned()),
            AlertError::IndicatorSpec(source) => Self::BadRequest(source.to_string()),
            AlertError::Database(source) => {
                tracing::error!(%source, "alert store: database error");
                Self::Internal
            }
            // `CorruptInstrumentId`/`CorruptTimeframe`/`CorruptCondition`,
            // plus any future `#[non_exhaustive]` variant — same reasoning
            // as `WorkspaceError`'s corrupt-row arm above.
            other => {
                tracing::error!(?other, "alert store: unmapped error variant");
                Self::Internal
            }
        }
    }
}

/// `watchlist_handlers`' translation from `senken_watchlist::WatchlistStore`'s
/// error type, mirroring [`From<senken_chart::ChartError>`] exactly — the
/// same guarded-store shape, the same "the store's `Identity` error reuses
/// this crate's own `IdentityError` mapping" reasoning. A missing group or
/// member is `BadRequest`, not a `404`, the same choice this crate already
/// makes for `ChartError::WorkspaceNotFound`/`AlertError::AlertNotFound` —
/// `HandlerError` has no `NotFound` variant, and introducing one for this
/// store alone would make "no such row" mean two different statuses
/// depending on which resource a caller asked about.
impl From<senken_watchlist::WatchlistError> for HandlerError {
    fn from(error: senken_watchlist::WatchlistError) -> Self {
        use senken_watchlist::WatchlistError;
        match error {
            WatchlistError::Identity(source) => source.into(),
            WatchlistError::GroupNotFound => Self::BadRequest("no such watchlist group".to_owned()),
            WatchlistError::MemberNotFound => {
                Self::BadRequest("no such watchlist member".to_owned())
            }
            WatchlistError::Database(source) => {
                tracing::error!(%source, "watchlist store: database error");
                Self::Internal
            }
            // `CorruptInstrument`, plus any future `#[non_exhaustive]`
            // variant: a stored row this build cannot even parse back is
            // this crate's own bug or a hand-edited database, never
            // something a caller's request could have caused — same
            // reasoning as `ChartError`'s corrupt-row arm.
            other => {
                tracing::error!(?other, "watchlist store: unmapped error variant");
                Self::Internal
            }
        }
    }
}

/// `notes_handlers`' translation from `senken_notes::NoteStore`'s error
/// type, mirroring [`From<senken_watchlist::WatchlistError>`] exactly.
impl From<senken_notes::NoteError> for HandlerError {
    fn from(error: senken_notes::NoteError) -> Self {
        use senken_notes::NoteError;
        match error {
            NoteError::Identity(source) => source.into(),
            NoteError::NoteNotFound => Self::BadRequest("no such note".to_owned()),
            NoteError::Database(source) => {
                tracing::error!(%source, "note store: database error");
                Self::Internal
            }
            // Any future `#[non_exhaustive]` variant fails closed the same
            // way every other guarded store's mapping here does.
            other => {
                tracing::error!(?other, "note store: unmapped error variant");
                Self::Internal
            }
        }
    }
}

impl axum::response::IntoResponse for HandlerError {
    fn into_response(self) -> axum::response::Response {
        use axum::Json;
        use axum::http::StatusCode;

        let (status, message) = match self {
            Self::BadRequest(message) => (StatusCode::BAD_REQUEST, message),
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_owned()),
            Self::InvalidCredentials => (
                StatusCode::UNAUTHORIZED,
                "that email and password do not match an account".to_owned(),
            ),
            Self::Forbidden(message) => (StatusCode::FORBIDDEN, message),
            Self::Conflict(message) => (StatusCode::CONFLICT, message),
            Self::TooManyRequests => (
                StatusCode::TOO_MANY_REQUESTS,
                "too many attempts, try again later".to_owned(),
            ),
            Self::Internal => (
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal server error".to_owned(),
            ),
        };
        (status, Json(crate::dto::ErrorBody::new(message))).into_response()
    }
}
