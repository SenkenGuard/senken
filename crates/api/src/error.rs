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
    /// `502 Bad Gateway`: an upstream this server depends on could not be
    /// reached, so **the request's outcome is unknown**.
    ///
    /// Distinct from [`Internal`](Self::Internal), which says this server
    /// failed and nothing happened. A trading request that timed out on its
    /// way to a venue may still have been accepted there, and a client must
    /// reconcile rather than retry — which it can only do if the two cases
    /// are told apart.
    BadGateway(String),
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

/// `trade_handlers`' translation from `senken_trade`'s error type.
///
/// Three of these arms are worth more than the status they map to.
/// `UnknownAccount` is deliberately not a `403`: an account someone else
/// owns must be indistinguishable from one that does not exist, which is
/// why the store returns that variant rather than a forbidden in the first
/// place. `Transport` gets its own status and its own wording because the
/// request's fate is genuinely unknown — reporting it as an ordinary
/// failure would tell a user nothing happened when an order may well have
/// reached the venue. And `NoMarkPrice` names the fix, because loading
/// history for the instrument is something the user can actually do.
///
/// A missing account or order is `BadRequest`, not a `404`, the same choice
/// this crate already makes for every other store — see
/// [`From<senken_watchlist::WatchlistError>`]'s own note on why.
impl From<senken_trade::TradeError> for HandlerError {
    fn from(error: senken_trade::TradeError) -> Self {
        use senken_trade::TradeError;
        match error {
            TradeError::Identity(source) => source.into(),
            TradeError::Settings(source) => Self::BadRequest(source.to_string()),
            TradeError::UnknownAccount => Self::BadRequest("no such trading account".to_owned()),
            TradeError::UnknownOrder => Self::BadRequest("no such order".to_owned()),
            TradeError::UnknownPosition(instrument) => {
                Self::BadRequest(format!("no open position for {instrument}"))
            }
            // A stale row a client tried to close after the position went
            // away is the client's answer to hear, not an internal error:
            // it means "already gone", and refreshing shows why.
            TradeError::UnknownPositionId(_) => {
                Self::BadRequest("that position is no longer open".to_owned())
            }
            TradeError::UnknownAdapter(id) => {
                Self::BadRequest(format!("no trade adapter `{id}` is registered"))
            }
            TradeError::DuplicateLabel => Self::Conflict(
                "you already have an account with that name on this adapter".to_owned(),
            ),
            TradeError::AccountDisabled => {
                Self::Forbidden("this account is switched off".to_owned())
            }
            TradeError::ReadOnly { note, .. } => {
                Self::Forbidden(note.unwrap_or_else(|| "this account is read-only".to_owned()))
            }
            TradeError::OrderNotOpen => Self::Conflict("that order is no longer open".to_owned()),
            TradeError::NoMarkPrice(instrument) => Self::BadRequest(format!(
                "no price is available for {instrument} yet — load some history for it first"
            )),
            TradeError::UnknownInstrument(instrument) => {
                Self::BadRequest(format!("no instrument `{instrument}` is catalogued"))
            }
            TradeError::InstrumentNotTradable {
                adapter,
                instrument,
            } => Self::BadRequest(format!("`{adapter}` does not trade {instrument}")),
            // Both are the caller's request being wrong in a way only the
            // adapter could know, and both already carry wording written
            // for the person reading it.
            TradeError::InvalidRequest(reason) | TradeError::InsufficientBalance(reason) => {
                Self::BadRequest(reason)
            }
            TradeError::Unsupported {
                adapter,
                capability,
            } => Self::BadRequest(format!("`{adapter}` does not support {capability}")),
            TradeError::Rejected(reason) => {
                Self::BadRequest(format!("the venue refused this: {reason}"))
            }
            TradeError::Transport { source } => {
                tracing::error!(%source, "trade adapter transport failure");
                Self::BadGateway(
                    "the venue could not be reached, so this request's outcome is unknown —                      check the account before retrying"
                        .to_owned(),
                )
            }
            TradeError::Database(source) => {
                tracing::error!(%source, "trade account store: database error");
                Self::Internal
            }
            TradeError::Adapter { source } => {
                tracing::error!(%source, "trade adapter failure");
                Self::Internal
            }
            other => {
                tracing::error!(?other, "trade engine: unmapped error variant");
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

/// `dashboard_handlers`' translation from
/// `senken_dashboard::DashboardError`, mirroring
/// [`From<senken_chart::ChartError>`] exactly — the same guarded-store
/// shape, the same "the store's `Identity` error reuses this crate's own
/// `IdentityError` mapping" reasoning. `RevisionConflict` is this store's
/// one addition over the others: a `409`, the same status
/// [`HandlerError::Conflict`] already carries for an already-registered
/// email, since both mean "the request was well-formed, but the state it
/// assumed no longer holds."
impl From<senken_dashboard::DashboardError> for HandlerError {
    fn from(error: senken_dashboard::DashboardError) -> Self {
        use senken_dashboard::DashboardError;
        match error {
            DashboardError::Identity(source) => source.into(),
            DashboardError::WorkspaceNotFound => {
                Self::BadRequest("no such dashboard workspace".to_owned())
            }
            DashboardError::WidgetNotFound => {
                Self::BadRequest("no such widget in this workspace".to_owned())
            }
            DashboardError::RevisionConflict { expected, current } => Self::Conflict(format!(
                "expected revision {expected}, but the workspace is now at revision {current}"
            )),
            DashboardError::OutOfBounds {
                x,
                y,
                width,
                columns,
            } => Self::BadRequest(format!(
                "widget at ({x}, {y}) with width {width} does not fit within {columns} columns"
            )),
            DashboardError::ZeroSizedWidget { x, y } => {
                Self::BadRequest(format!("widget at ({x}, {y}) has zero width or height"))
            }
            DashboardError::OverlappingWidgets { x1, y1, x2, y2 } => {
                Self::BadRequest(format!("widgets at ({x1}, {y1}) and ({x2}, {y2}) overlap"))
            }
            DashboardError::InvalidConfig(reason) => {
                Self::BadRequest(format!("widget config is not valid JSON: {reason}"))
            }
            DashboardError::EmptyWidgetIdentity => Self::BadRequest(
                "widget provider_id and widget_type_id must not be empty".to_owned(),
            ),
            DashboardError::Database(source) => {
                tracing::error!(%source, "dashboard store: database error");
                Self::Internal
            }
            // Any future `#[non_exhaustive]` variant fails closed the same
            // way every other guarded store's mapping here does.
            other => {
                tracing::error!(?other, "dashboard store: unmapped error variant");
                Self::Internal
            }
        }
    }
}

/// `widget_plugin_handlers`' translation from
/// `senken_plugin::widget_package::WidgetPackageError`. Like
/// `senken_store::StoreError` below, this has no `Identity` variant to
/// defer to — the store itself has no notion of a user, so
/// `widget_plugin_handlers` calls `AuthenticatedUser::authorize` itself
/// before ever reaching the store.
impl From<senken_plugin::widget_package::WidgetPackageError> for HandlerError {
    fn from(error: senken_plugin::widget_package::WidgetPackageError) -> Self {
        use senken_plugin::widget_package::WidgetPackageError;
        match error {
            WidgetPackageError::NotFound(id) => Self::BadRequest(format!(
                "no widget plugin package with id {id:?} is installed"
            )),
            WidgetPackageError::Manifest(source) => {
                Self::BadRequest(format!("package manifest is invalid: {source}"))
            }
            WidgetPackageError::InvalidArchive(reason) => {
                Self::BadRequest(format!("package archive is invalid: {reason}"))
            }
            WidgetPackageError::UnsafeArchiveEntry(name) => {
                Self::BadRequest(format!("package archive contains an unsafe path: {name:?}"))
            }
            WidgetPackageError::MissingManifest => {
                Self::BadRequest("package archive has no manifest.json at its root".to_owned())
            }
            WidgetPackageError::EntryNotFound(widget_id, entry) => Self::BadRequest(format!(
                "widget {widget_id:?} names entry {entry:?}, which the archive does not contain"
            )),
            WidgetPackageError::UnsafeAssetPath(path) => {
                Self::BadRequest(format!("asset path {path:?} is not a valid relative path"))
            }
            WidgetPackageError::Storage(source) => {
                tracing::error!(%source, "widget plugin store: storage error");
                Self::Internal
            }
            WidgetPackageError::Io(source) => {
                tracing::error!(%source, "widget plugin store: io error");
                Self::Internal
            }
        }
    }
}

/// `storage_handlers`' translation from `senken_store::StoreError`.
/// Unlike every other guarded-store mapping in this file, this one has no
/// `Identity` variant to defer to — `senken-store` has no notion of a
/// user, so `storage_handlers` calls `AuthenticatedUser::authorize` itself
/// and maps that separately.
impl From<senken_store::StoreError> for HandlerError {
    fn from(error: senken_store::StoreError) -> Self {
        use senken_store::StoreError;
        match error {
            // The one caller-triggerable case: a `source_id`/`symbol`/
            // `series_id` that is not a single safe path segment.
            StoreError::InvalidPathSegment(segment) => {
                Self::BadRequest(format!("{segment:?} is not a valid path segment"))
            }
            StoreError::Io { path, source } => {
                tracing::error!(?path, %source, "store: io error");
                Self::Internal
            }
            StoreError::Storage(source) => {
                tracing::error!(%source, "store: storage error");
                Self::Internal
            }
            // `Rejected`/`Parquet`/`Arrow` only ever arise on the
            // read/write-bars path, never on usage accounting or deletion —
            // reachable here only if `senken-store` grows a new caller of
            // this same error type, so they fail closed like any other
            // unmapped variant.
            other => {
                tracing::error!(?other, "store: unmapped error variant");
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
            Self::BadGateway(message) => (StatusCode::BAD_GATEWAY, message),
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
