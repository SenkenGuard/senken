//! `POST /api/my/indicators` and friends — indicators an account wrote and
//! compiled themselves.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_indicator_registry::{UserIndicator, UserIndicatorSummary};

/// A row from `GET /api/my/indicators` — never the source, which
/// `GET /api/my/indicators/{id}` alone carries (the same "listing never
/// carries the heavy field" shape `NoteSummaryDto` uses for a note's body).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UserIndicatorSummaryDto {
    /// The indicator's id.
    pub id: String,
    /// The catalog slug this indicator compiles to (`my/<slug>`).
    pub slug: String,
    /// The display title, as typed by its author.
    pub title: String,
    /// `true` when the most recent compile attempt succeeded and a
    /// compiled component exists (which may predate the currently saved
    /// source — see `compile_error`).
    pub compiled: bool,
    /// The most recent compile attempt's error message, if it failed.
    /// `None` when the last attempt succeeded, or before any attempt.
    pub compile_error: Option<String>,
    /// When the row was last changed, Unix nanoseconds.
    pub updated_at: i64,
}

impl From<UserIndicatorSummary> for UserIndicatorSummaryDto {
    fn from(summary: UserIndicatorSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            slug: summary.slug,
            title: summary.title,
            compiled: summary.compiled,
            compile_error: summary.compile_error,
            updated_at: summary.updated_at.as_nanos(),
        }
    }
}

/// `GET /api/my/indicators/{id}` response body: everything in
/// [`UserIndicatorSummaryDto`], plus the source. Never the compiled
/// component's bytes — nothing in the client needs them, and a component
/// can be several kilobytes of binary with no reason to cross this wire.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UserIndicatorDto {
    /// The indicator's id.
    pub id: String,
    /// The catalog slug this indicator compiles to (`my/<slug>`).
    pub slug: String,
    /// The display title, as typed by its author.
    pub title: String,
    /// `true` when the most recent compile attempt succeeded and a
    /// compiled component exists.
    pub compiled: bool,
    /// The most recent compile attempt's error message, if it failed.
    pub compile_error: Option<String>,
    /// When the row was last changed, Unix nanoseconds.
    pub updated_at: i64,
    /// The Rust source as last saved.
    pub source: String,
    /// The `senken-plugin-api` version the current compiled component was
    /// built against, if one has ever compiled successfully.
    pub api_version: Option<String>,
}

impl From<UserIndicator> for UserIndicatorDto {
    fn from(indicator: UserIndicator) -> Self {
        Self {
            id: indicator.id.to_string(),
            slug: indicator.slug,
            title: indicator.title,
            compiled: indicator.wasm.is_some(),
            compile_error: indicator.compile_error,
            updated_at: indicator.updated_at.as_nanos(),
            source: indicator.source,
            api_version: indicator.api_version,
        }
    }
}

/// `POST /api/my/indicators` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateUserIndicatorRequest {
    /// The indicator's display title — its catalog slug is derived from
    /// this.
    pub title: String,
    /// The Rust source to compile.
    pub source: String,
}

/// `PUT /api/my/indicators/{id}` request body. `title` is optional: a save
/// that only edits code sends `source` alone and keeps the existing title
/// (and slug).
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct UpdateUserIndicatorRequest {
    /// A new display title, if the author renamed it.
    pub title: Option<String>,
    /// The Rust source to compile.
    pub source: String,
}

/// One error-level diagnostic from a failed compile, with the line/column
/// in the author's own source `rustc` could resolve one to. Never carries
/// `rustc`'s fully rendered text — that can name a path on the server —
/// only the diagnostic's own short message; the full text goes to the
/// server log instead.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UserIndicatorDiagnosticDto {
    /// 1-based line in the author's `src/lib.rs`, if `rustc` named one.
    pub line: Option<u32>,
    /// 1-based column, if `rustc` named one.
    pub column: Option<u32>,
    /// The diagnostic's short message.
    pub message: String,
}

impl From<senken_indicator_compile::Diagnostic> for UserIndicatorDiagnosticDto {
    fn from(diagnostic: senken_indicator_compile::Diagnostic) -> Self {
        Self {
            line: diagnostic.line,
            column: diagnostic.column,
            message: diagnostic.message,
        }
    }
}

/// `POST`/`PUT /api/my/indicators*`'s response body: whether the save's own
/// compile attempt (create, save, or an explicit recompile) succeeded, and
/// why not if it did not. A failed compile is still a `200` — the source
/// was accepted and stored either way — never a `4xx`/`5xx` for a mistake
/// in the author's own Rust.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SaveUserIndicatorResponse {
    /// The indicator's id.
    pub id: String,
    /// `true` if this attempt compiled successfully.
    pub compiled: bool,
    /// Present, and non-empty, only when `compiled` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diagnostics: Option<Vec<UserIndicatorDiagnosticDto>>,
}

/// `GET /api/my/indicators/toolchain` response body — whether this server
/// can compile a Rust indicator at all right now, so the panel can disable
/// Save with a reason instead of letting a save fail with no explanation.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorToolchainStatusResponse {
    /// `true` if a Rust toolchain with the `wasm32-wasip2` target was
    /// found when this server started.
    pub available: bool,
    /// Why not, if `available` is `false`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}
