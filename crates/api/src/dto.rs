//! Request and response bodies for every endpoint this crate mounts (plan
//! 004 its auth surface, the workspace/bars/indicators/alerts
//! surface).
//!
//! Every shape here derives both [`serde`]'s traits (what the handlers
//! actually (de)serialise) and [`utoipa::ToSchema`] (the "`OpenAPI`"
//! section: `utoipa` derives the document from these same structs, so a
//! field renamed here is a compile error in the generated TypeScript, not a
//! runtime surprise for Q5/Q6). There is deliberately no second,
//! hand-written shape anywhere else.

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_acl::Grant;
use senken_alerts::{AlertRecord, Comparator, Condition, IndicatorField, IndicatorSpec};
use senken_core::TimeRange;
use senken_identity::{RoleSummary, UserSummary};
use senken_loader::{JobSnapshot, Phase, Requirement};
use senken_marketdata::{InstrumentKind, InstrumentMatch};
use senken_series::Bar;
use senken_workspace::{
    DrawingKind, DrawingLineStyle, DrawingPoint, DrawingRecord, LayerKind, LayerRecord,
    LayoutDetail, LayoutSummary, PaneRecord, WorkspaceSummary,
};

/// `POST /api/login` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct LoginRequest {
    /// The account's email.
    pub email: String,
    /// The account's password, in plain text over an already-authenticated
    /// transport (`Authorization: Bearer`, not a cookie, so there is no separate CSRF surface to protect this call from).
    pub password: String,
}

/// `POST /api/login` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LoginResponse {
    /// The freshly minted session token. The client attaches this as
    /// `Authorization: Bearer <token>` on every later request — it is never placed in a cookie or a URL.
    pub token: String,
}

/// `POST /api/set-password` request body.
///
/// `email` is required only for the anonymous first-run path:
/// an authenticated caller changing their own password is identified by
/// their session instead, and any `email` they supply is ignored — there is
/// no self-service way to name a *different* account here.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct SetPasswordRequest {
    /// The account to set a password for. Required when the request carries
    /// no `Authorization` header; ignored when it does.
    #[serde(default)]
    pub email: Option<String>,
    /// The new password (length floor only, checked by `senken-identity`).
    pub new_password: String,
}

/// `GET /api/me` response body: the caller's own profile, plus the roles and effective grants that let the client cosmetically hide
/// admin sections. **UI convenience only**: every
/// endpoint re-checks a real grant on every request regardless of what this
/// response says.
///
/// This is also the endpoint a client should poll to know
/// whether it is really still authenticated, instead of `GET /api/health`:
/// `health` needs no credential at all, so a successful poll of it reads as
/// "authenticated" whether or not a session exists, which is exactly the
/// bug Q3 flagged and Q6 hit live. `me` requires
/// [`crate::auth::EndpointPermission::Authenticated`], so a `200` here
/// really does mean a live, unfenced session, and a `401` really does mean
/// the credential is gone.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct MeResponse {
    /// The account's id.
    pub id: String,
    /// The account's email.
    pub email: String,
    /// The account's display name.
    pub display_name: String,
    /// `true` if the account is disabled.
    pub disabled: bool,
    /// `true` once the account has set a password. Always `true` here in
    /// practice — a session can only be resolved for an account after it
    /// exists, and the fence gate means `/api/me` itself already refused a
    /// fenced account's session — but reported anyway rather than assumed,
    /// so a client never has to special-case why the field is missing.
    pub password_set: bool,
    /// The names of every role this account holds.
    pub roles: Vec<String>,
    /// This account's effective grants: every `(Action, Resource, Scope)`
    /// it holds through any role or direct grant, one entry per
    /// `(Action, Resource)` pair at the widest scope granted for it (see
    /// `senken_identity::AuthenticatedUser::effective_grants`).
    pub grants: Vec<GrantDto>,
}

/// A `senken_acl::Grant` as this crate serialises it: three fields, each
/// carrying its `senken_acl` enum value directly (already `Serialize`,
/// since `senken-acl` derives it) but documented to `utoipa` as a plain
/// string via `#[schema(value_type = String)]`. That attribute is needed,
/// not cosmetic: `Action`/`Resource`/`Scope` cannot derive `utoipa::ToSchema`
/// themselves without editing `senken-acl` (out of this crate's owned
/// paths, consumed as-is), and Rust's orphan
/// rule forbids implementing that foreign trait for that foreign type from
/// here. The wire format is exactly each enum's variant name (`"View"`,
/// `"Workspace"`, `"Own"`, …) — the same spelling
/// `packages/web/src/lib/components/settings/sections/access-section.svelte`
/// already mirrors from `crates/acl/src/{action,resource,scope}.rs` for its
/// (currently disabled) grant matrix.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct GrantDto {
    /// What the grant permits doing.
    #[schema(value_type = String)]
    pub action: senken_acl::Action,
    /// What it permits doing it to.
    #[schema(value_type = String)]
    pub resource: senken_acl::Resource,
    /// How far that permission reaches.
    #[schema(value_type = String)]
    pub scope: senken_acl::Scope,
}

impl From<Grant> for GrantDto {
    fn from(grant: Grant) -> Self {
        Self {
            action: grant.action,
            resource: grant.resource,
            scope: grant.scope,
        }
    }
}

impl From<GrantDto> for Grant {
    fn from(dto: GrantDto) -> Self {
        Self::new(dto.action, dto.resource, dto.scope)
    }
}

/// A response body carrying only a freshly created row's id (`POST /api/users`, `POST /api/roles`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IdResponse {
    /// The id of the row just created.
    pub id: String,
}

/// A user row as the user/role management endpoints report it — the same fields as [`MeResponse`]'s profile half, without the
/// roles/grants that are specific to *the caller's own* identity.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UserSummaryDto {
    /// The user's id.
    pub id: String,
    /// The user's email.
    pub email: String,
    /// The user's display name.
    pub display_name: String,
    /// `true` if the account is disabled.
    pub disabled: bool,
    /// `true` once the account has set a password.
    pub password_set: bool,
}

impl From<UserSummary> for UserSummaryDto {
    fn from(summary: UserSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            email: summary.email,
            display_name: summary.display_name,
            disabled: summary.disabled,
            password_set: summary.password_set,
        }
    }
}

/// `GET /api/users` response body (scope reaches the
/// query, including this `total`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct UsersPage {
    /// The rows for this page.
    pub rows: Vec<UserSummaryDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// A role row as the user/role management endpoints report it, including the grants it carries so the client can render the grant
/// matrix `access-section.svelte` already built, disabled, against this
/// exact shape.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RoleSummaryDto {
    /// The role's id.
    pub id: String,
    /// The role's name.
    pub name: String,
    /// A human-readable description.
    pub description: String,
    /// `true` for a role seeded by `senken-identity` (e.g. `Superadmin`)
    /// rather than created by an admin.
    pub builtin: bool,
    /// The grants this role carries.
    pub grants: Vec<GrantDto>,
}

impl From<RoleSummary> for RoleSummaryDto {
    fn from(summary: RoleSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            name: summary.name,
            description: summary.description,
            builtin: summary.builtin,
            grants: summary.grants.into_iter().map(GrantDto::from).collect(),
        }
    }
}

/// `GET /api/roles` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct RolesPage {
    /// The rows for this page.
    pub rows: Vec<RoleSummaryDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// `POST /api/users` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateUserRequest {
    /// The new account's email.
    pub email: String,
    /// The new account's display name.
    pub display_name: String,
    /// An initial password. Omitted (or `null`) leaves the account behind
    /// the same B4 fence the default admin is seeded with, so the new user
    /// sets their own password on first use.
    #[serde(default)]
    pub initial_password: Option<String>,
}

/// `POST /api/roles` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateRoleRequest {
    /// The role's name.
    pub name: String,
    /// A human-readable description.
    #[serde(default)]
    pub description: String,
    /// The grants this role carries (a role is a named set of
    /// `(Action, Resource, Scope)` triples, never free text).
    #[serde(default)]
    pub grants: Vec<GrantDto>,
}

/// `POST /api/users/{user_id}/roles` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct AssignRoleRequest {
    /// The role to assign, as its id's `Display` form.
    pub role_id: String,
}

/// `POST /api/users/{user_id}/plugin-grants` (and its `role_id`/`.../revoke`
/// siblings) request body: a plugin permission is granted
/// or revoked whole, by name — never interpreted, unlike a core [`GrantDto`].
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct PluginGrantRequest {
    /// The permission's full name, `<plugin-id>.<resource>:<operation>`.
    pub name: String,
}

/// `POST /api/ws/ticket` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WsTicketResponse {
    /// A single-use, seconds-lived ticket. Presented once on the WebSocket
    /// handshake's query string (safe specifically because it is single-use
    /// and short-lived, unlike a session token) and discarded by the server
    /// on redemption. Matches `packages/web/src/lib/api/websocket.ts`'s
    /// `WsTicketResponse` field-for-field.
    pub ticket: String,
}

/// The body of every non-2xx JSON error response this crate returns.
///
/// One shape for every failure (the login requirement —
/// "identical response... for unknown-user and wrong-password" —
/// generalised to every endpoint): a caller branches on the HTTP status,
/// never on the body's contents.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ErrorBody {
    /// A human-readable explanation. Never account-specific detail that
    /// would help enumerate accounts or diagnose *why* a request was
    /// rejected beyond what the status code already says.
    pub error: String,
}

impl ErrorBody {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            error: message.into(),
        }
    }
}

// ============================================================================
// Workspaces, layouts, panes and layers.
// ============================================================================

/// A workspace row (list/create/rename/delete).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WorkspaceDto {
    /// The workspace's id.
    pub id: String,
    /// The account that owns this workspace.
    pub owner_id: String,
    /// The workspace's display name.
    pub name: String,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to the workspace's own fields.
    pub updated_at: i64,
}

impl From<WorkspaceSummary> for WorkspaceDto {
    fn from(summary: WorkspaceSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            owner_id: summary.owner_id.to_string(),
            name: summary.name,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
        }
    }
}

/// `GET /api/workspaces` response body (scope reaches the query, including this `total`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct WorkspacesPage {
    /// The rows for this page.
    pub rows: Vec<WorkspaceDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// `POST /api/workspaces` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateWorkspaceRequest {
    /// The new workspace's display name.
    pub name: String,
}

/// `PATCH /api/workspaces/{id}` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RenameWorkspaceRequest {
    /// The workspace's new display name.
    pub name: String,
}

/// `GET /api/workspaces/default` response body ("default-on- first-open belongs on the server, not in the client, so it holds for any client that ever connects").
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DefaultWorkspaceResponse {
    /// The caller's default workspace, created on first call and returned
    /// unchanged on every later one.
    pub workspace_id: String,
    /// That workspace's one starting layout.
    pub layout_id: String,
}

/// A layout row without its panes (the "layouts ... nested
/// under" a workspace) — see [`LayoutDetailDto`] for the full nested shape.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LayoutSummaryDto {
    /// The layout's id.
    pub id: String,
    /// The workspace this layout belongs to.
    pub workspace_id: String,
    /// The layout's display name.
    pub name: String,
    /// The pane-grid arrangement, as one of the tokens
    /// `senken_workspace::LayoutPreset` round-trips through its own
    /// `Display`/`FromStr` (`"1"`, `"2h"`, `"2v"`, `"3h"`, `"3v"`, `"4"`).
    pub preset: String,
    /// This layout's tab order within its workspace.
    pub position: u32,
}

impl From<LayoutSummary> for LayoutSummaryDto {
    fn from(summary: LayoutSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            workspace_id: summary.workspace_id.to_string(),
            name: summary.name,
            preset: summary.preset.to_string(),
            position: summary.position,
        }
    }
}

/// One layer's kind, on the wire — mirrors `senken_workspace::LayerKind`
/// field-for-field. `params` stays an opaque JSON-text string rather than a
/// parsed value, the same way `senken_alerts::IndicatorSpec::params` already
/// does on the wire below: this crate does not interpret it either (plan
/// 005 its own scope note — that is the indicator engine's job).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum LayerKindDto {
    /// A second instrument plotted on the same pane as the main one.
    OverlayInstrument {
        /// The overlaid instrument, `source:symbol`.
        instrument: String,
    },
    /// An indicator drawn on top of the pane's main price plot.
    IndicatorOverlay {
        /// The indicator's name (see `GET /api/indicators`).
        name: String,
        /// The indicator's parameters, as JSON-object text.
        params: String,
    },
    /// An indicator drawn in its own pane beneath the main one.
    IndicatorSubPane {
        /// The indicator's name.
        name: String,
        /// The indicator's parameters, as JSON-object text.
        params: String,
    },
}

impl From<LayerKind> for LayerKindDto {
    fn from(kind: LayerKind) -> Self {
        match kind {
            LayerKind::OverlayInstrument { instrument } => Self::OverlayInstrument {
                instrument: instrument.as_str().to_owned(),
            },
            LayerKind::IndicatorOverlay { name, params } => Self::IndicatorOverlay { name, params },
            LayerKind::IndicatorSubPane { name, params } => Self::IndicatorSubPane { name, params },
        }
    }
}

/// One layer, as read back from a layout.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LayerDto {
    /// The layer's id.
    pub id: String,
    /// This layer's stacking order within its pane.
    pub position: u32,
    /// What kind of layer this is.
    pub kind: LayerKindDto,
    /// Whether the layer is currently shown.
    pub visible: bool,
    /// How this layer is drawn — colour, line style, width, per-plot
    /// visibility — as opaque JSON-object text. The server stores and
    /// returns it without interpreting it.
    pub style: String,
}

impl From<LayerRecord> for LayerDto {
    fn from(record: LayerRecord) -> Self {
        Self {
            id: record.id.to_string(),
            position: record.position,
            kind: record.kind.into(),
            visible: record.visible,
            style: record.style,
        }
    }
}

/// One layer to write, supplied as part of a [`PaneInputDto`].
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct LayerInputDto {
    /// This layer's stacking order within its pane.
    pub position: u32,
    /// What kind of layer this is.
    pub kind: LayerKindDto,
    /// Whether the layer starts out visible.
    pub visible: bool,
    /// How this layer is drawn, as opaque JSON-object text. Defaults to an
    /// empty object, which is what a layer whose styling has never been
    /// touched carries.
    #[serde(default = "empty_json_object")]
    pub style: String,
}

/// One (time, price) anchor for a [`DrawingKindDto::TrendLine`] or
/// [`DrawingKindDto::Rectangle`], on the wire.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct DrawingPointDto {
    /// Unix nanoseconds, matching every other time value this crate puts on
    /// the wire (`BarDto`'s `ts_open`, `TimeRangeDto`'s own fields).
    pub time: i64,
    /// The anchor's price — a coordinate the user set by clicking a chart,
    /// not an order price.
    pub price: f64,
}

impl From<DrawingPoint> for DrawingPointDto {
    fn from(point: DrawingPoint) -> Self {
        Self {
            time: point.time.as_nanos(),
            price: point.price,
        }
    }
}

/// One drawing's kind and geometry, on the wire — mirrors
/// `senken_workspace::DrawingKind` field-for-field.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum DrawingKindDto {
    /// A single price level, drawn across the pane's full width.
    HorizontalLine {
        /// The line's price level.
        price: f64,
    },
    /// A straight line between two points.
    TrendLine {
        /// The line's first point.
        start: DrawingPointDto,
        /// The line's second point.
        end: DrawingPointDto,
    },
    /// A filled zone between two corners.
    Rectangle {
        /// One corner.
        start: DrawingPointDto,
        /// The opposite corner.
        end: DrawingPointDto,
    },
}

impl From<DrawingKind> for DrawingKindDto {
    fn from(kind: DrawingKind) -> Self {
        match kind {
            DrawingKind::HorizontalLine { price } => Self::HorizontalLine { price },
            DrawingKind::TrendLine { start, end } => Self::TrendLine {
                start: start.into(),
                end: end.into(),
            },
            DrawingKind::Rectangle { start, end } => Self::Rectangle {
                start: start.into(),
                end: end.into(),
            },
        }
    }
}

/// A drawing's line style, on the wire — matches the exact vocabulary
/// `packages/web/src/lib/mock/chart-settings.ts`'s `LineStyle` already uses
/// for a plot's own line style, so a client never translates between two
/// spellings of the same three values.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub(crate) enum DrawingLineStyleDto {
    /// An unbroken line.
    Solid,
    /// A dashed line.
    Dashed,
    /// A dotted line.
    Dotted,
}

impl From<DrawingLineStyle> for DrawingLineStyleDto {
    fn from(style: DrawingLineStyle) -> Self {
        match style {
            DrawingLineStyle::Solid => Self::Solid,
            DrawingLineStyle::Dashed => Self::Dashed,
            DrawingLineStyle::Dotted => Self::Dotted,
        }
    }
}

/// One drawing, as read back from a layout.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DrawingDto {
    /// The drawing's id.
    pub id: String,
    /// This drawing's stacking order within its pane.
    pub position: u32,
    /// What kind of drawing this is, and its geometry.
    pub kind: DrawingKindDto,
    /// The drawing's colour, as `#rrggbb`.
    pub color: String,
    /// Stroke width in pixels, 1 through 4.
    pub width: u8,
    /// The drawing's line style.
    pub line_style: DrawingLineStyleDto,
}

impl From<DrawingRecord> for DrawingDto {
    fn from(record: DrawingRecord) -> Self {
        Self {
            id: record.id.to_string(),
            position: record.position,
            kind: record.kind.into(),
            color: record.style.color,
            width: record.style.width,
            line_style: record.style.line_style.into(),
        }
    }
}

/// One drawing to write, supplied as part of a [`PaneInputDto`].
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct DrawingInputDto {
    /// This drawing's stacking order within its pane.
    pub position: u32,
    /// What kind of drawing this is, and its geometry.
    pub kind: DrawingKindDto,
    /// The drawing's colour, as `#rrggbb`.
    pub color: String,
    /// Stroke width in pixels, 1 through 4.
    pub width: u8,
    /// The drawing's line style.
    pub line_style: DrawingLineStyleDto,
}

/// One pane, as read back from a layout.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct PaneDto {
    /// The pane's id.
    pub id: String,
    /// This pane's slot within its layout's grid.
    pub position: u32,
    /// The pane's main instrument, `source:symbol`.
    pub instrument: String,
    /// The main instrument's bar timeframe, e.g. `"1h"`.
    pub timeframe: String,
    /// This pane's layers, in stacking order.
    pub layers: Vec<LayerDto>,
    /// This pane's drawings, in stacking order.
    pub drawings: Vec<DrawingDto>,
    /// Display settings for this pane's chart as opaque JSON-object text.
    /// The server stores and returns it without interpreting it; the chart
    /// front end owns what these settings mean.
    pub settings: String,
}

impl From<PaneRecord> for PaneDto {
    fn from(record: PaneRecord) -> Self {
        Self {
            id: record.id.to_string(),
            position: record.position,
            instrument: record.instrument.as_str().to_owned(),
            timeframe: record.timeframe.to_string(),
            layers: record.layers.into_iter().map(LayerDto::from).collect(),
            drawings: record.drawings.into_iter().map(DrawingDto::from).collect(),
            settings: record.settings,
        }
    }
}

/// What one registered market-data source can do.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SourceCapabilityDto {
    /// The source id, e.g. `okx-spot` — the left half of an `InstrumentId`.
    pub id: String,
    /// The venue's own display name.
    pub name: String,
    /// A `BarSource` is registered for it, so its instruments can be charted.
    pub bars: bool,
    /// It has a live subscription pool, so its instruments can stream a
    /// price. Never true without `bars`.
    pub live: bool,
}

/// `GET /api/sources` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct SourcesResponse {
    /// Every registered source, ordered by id.
    pub sources: Vec<SourceCapabilityDto>,
}

/// One pane to write, supplied to `PUT /api/layouts/{id}`.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct PaneInputDto {
    /// This pane's slot within the layout's grid. Must be unique among the
    /// panes in one call.
    pub position: u32,
    /// The pane's main instrument, `source:symbol`.
    pub instrument: String,
    /// The main instrument's bar timeframe, e.g. `"1h"`.
    pub timeframe: String,
    /// This pane's layers.
    #[serde(default)]
    pub layers: Vec<LayerInputDto>,
    /// This pane's drawings.
    #[serde(default)]
    pub drawings: Vec<DrawingInputDto>,
    /// Display settings for this pane's chart as opaque JSON-object text.
    /// Defaults to an empty object, which is what a pane that has never had
    /// its settings touched carries.
    #[serde(default = "empty_json_object")]
    pub settings: String,
}

fn empty_json_object() -> String {
    "{}".to_owned()
}

/// A layout together with its panes and their layers — the full nested
/// structure workspace → layout → panes → layers, minus the workspace
/// itself.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LayoutDetailDto {
    /// The layout's identity and grid arrangement.
    pub layout: LayoutSummaryDto,
    /// The layout's panes, in grid-slot order.
    pub panes: Vec<PaneDto>,
}

impl From<LayoutDetail> for LayoutDetailDto {
    fn from(detail: LayoutDetail) -> Self {
        Self {
            layout: detail.layout.into(),
            panes: detail.panes.into_iter().map(PaneDto::from).collect(),
        }
    }
}

/// `PUT /api/layouts/{id}` request body: replaces a layout's entire pane/
/// layer structure in one transaction.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ReplaceLayoutRequest {
    /// The pane-grid arrangement, one of `senken_workspace::LayoutPreset`'s
    /// own tokens (`"1"`, `"2h"`, `"2v"`, `"3h"`, `"3v"`, `"4"`).
    pub preset: String,
    /// The layout's new panes. Length must match `preset`'s pane count.
    pub panes: Vec<PaneInputDto>,
}

// ============================================================================
// Bars.
// ============================================================================

/// `?instrument=&spec=&from=&to=` — shared by `GET /api/bars/plan` and
/// `GET /api/bars/range`. `from`/`to` are Unix nanoseconds
/// (`senken_core::UnixNanos`'s own wire representation — see that type's
/// docs — so no unit ambiguity crosses the HTTP boundary).
#[derive(Debug, Deserialize)]
pub(crate) struct BarRangeQuery {
    /// The instrument, `source:symbol`, e.g. `binance-spot:BTCUSDT`.
    pub instrument: String,
    /// The bar timeframe, e.g. `"1h"`.
    pub spec: String,
    /// Inclusive start of the range, Unix nanoseconds.
    pub from: i64,
    /// Exclusive end of the range, Unix nanoseconds.
    pub to: i64,
}

/// A half-open `[from, to)` span of time, on the wire — Unix nanoseconds at
/// both ends, matching [`BarRangeQuery`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct TimeRangeDto {
    /// Inclusive start, Unix nanoseconds.
    pub from: i64,
    /// Exclusive end, Unix nanoseconds.
    pub to: i64,
}

impl From<TimeRange> for TimeRangeDto {
    fn from(range: TimeRange) -> Self {
        Self {
            from: range.start().as_nanos(),
            to: range.end().as_nanos(),
        }
    }
}

/// `GET /api/bars/plan` response body (pure inspection, no side effects — see `senken_loader::SeriesLoader::plan`'s own docs).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BarsRequirementDto {
    /// The parts of the requested range already resolvable without
    /// fetching anything.
    pub covered: Vec<TimeRangeDto>,
    /// The parts a call to `POST /api/bars/ensure` would actually need to
    /// fetch.
    pub missing: Vec<TimeRangeDto>,
    /// How many venue-page-sized fetch chunks `missing` splits into.
    pub chunks: u32,
    /// An estimate of how many bars `missing` represents.
    pub estimated_bars: u64,
    /// An estimate, in seconds, of how long fetching `missing` would take —
    /// `None` until this loader has measured its own throughput.
    pub estimate_secs: Option<f64>,
}

impl From<Requirement> for BarsRequirementDto {
    fn from(requirement: Requirement) -> Self {
        Self {
            covered: requirement
                .covered
                .into_iter()
                .map(TimeRangeDto::from)
                .collect(),
            missing: requirement
                .missing
                .into_iter()
                .map(TimeRangeDto::from)
                .collect(),
            chunks: requirement.chunks,
            estimated_bars: requirement.estimated_bars,
            estimate_secs: requirement.estimate.map(|d| d.as_secs_f64()),
        }
    }
}

/// One OHLCV bar, on the wire: plain scaled integers, exactly
/// as stored — the series' price/quantity scale is not carried per bar (see
/// `senken_series::Bar`'s own docs), so a client must already know an
/// instrument's `price_scale`/`qty_scale` (from `senken search`/the catalog)
/// to render these as real prices.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BarDto {
    /// The start of the interval this bar covers, Unix nanoseconds.
    pub ts_open: i64,
    /// The first trade price in the interval, at the series' price scale.
    pub open: i64,
    /// The highest trade price in the interval.
    pub high: i64,
    /// The lowest trade price in the interval.
    pub low: i64,
    /// The last trade price in the interval.
    pub close: i64,
    /// Base-asset volume traded in the interval, at the series' quantity
    /// scale.
    pub volume: i64,
    /// Quote-asset volume, when the venue reports it.
    pub quote_volume: Option<i64>,
    /// Number of trades in the interval, when the venue reports it.
    pub trade_count: Option<u32>,
    /// Base-asset volume bought by takers, when the venue reports it.
    pub taker_buy_volume: Option<i64>,
}

impl From<&Bar> for BarDto {
    fn from(bar: &Bar) -> Self {
        Self {
            ts_open: bar.ts_open.as_nanos(),
            open: bar.open,
            high: bar.high,
            low: bar.low,
            close: bar.close,
            volume: bar.volume,
            quote_volume: bar.quote_volume,
            trade_count: bar.trade_count,
            taker_buy_volume: bar.taker_buy_volume,
        }
    }
}

/// `GET /api/bars/range` response body: whatever is already resolvable
/// right now (the "progressive delivery: never blocks on a
/// fetch"), plus what is not — call `POST /api/bars/ensure` to start
/// filling `missing`.
///
/// Carries `price_scale`/`qty_scale` alongside the bars themselves
/// : `BarDto`'s own doc already says a client must already know an
/// instrument's `price_scale`/`qty_scale` to render these as real prices,
/// but nothing in the surface exposes a catalog lookup for
/// them — no `GET /api/instruments` exists, deliberately (this names exactly
/// four route groups: workspaces, bars, indicators, alerts). Rather than
/// leave the charts page with raw scaled integers and no way to turn them
/// into a price, this response carries the one instrument's `price_scale`/
/// `qty_scale` it already resolved to serve the request.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BarRangeResponse {
    /// Bars already available, ascending by `ts_open`.
    pub bars: Vec<BarDto>,
    /// Ranges not yet resolvable.
    pub missing: Vec<TimeRangeDto>,
    /// Decimal places in a price — see `senken_marketdata::Instrument`'s own
    /// fixed-point contract. Every `BarDto` field above is `value ×
    /// 10^price_scale`.
    pub price_scale: u8,
    /// Decimal places in a quantity, the same contract for `volume` et al.
    pub qty_scale: u8,
    /// When the bar currently forming closes and the next one opens, as
    /// nanoseconds since the Unix epoch.
    ///
    /// Supplied by the server because bucket boundaries depend on the
    /// series' anchor, which the client does not carry: a venue-supplied
    /// Day-or-above series can roll over hours away from UTC midnight, and a
    /// countdown computed from a UTC floor would be wrong by that much.
    pub next_bar_open_at: i64,
}

/// `POST /api/bars/ensure` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct EnsureBarsRequest {
    /// The instrument, `source:symbol`.
    pub instrument: String,
    /// The bar timeframe, e.g. `"1h"`.
    pub spec: String,
    /// Inclusive start of the range, Unix nanoseconds.
    pub from: i64,
    /// Exclusive end of the range, Unix nanoseconds.
    pub to: i64,
    /// `"background"`, `"prefetch"` or `"visible"` (the default) — matches
    /// `senken_loader::Priority`'s three variants exactly.
    #[serde(default)]
    pub priority: Option<String>,
}

/// `POST /api/bars/ensure` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct EnsureBarsResponse {
    /// Opaque; pass verbatim to `GET /api/bars/jobs/{job_id}`. Encodes which
    /// loader minted it — `senken_loader::JobId` is unique only within the
    /// loader that assigned it (see that type's own docs), never globally —
    /// so this is not simply that id's decimal text.
    pub job_id: String,
}

/// `GET /api/bars/jobs/{job_id}` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct BarJobDto {
    /// `"queued"`, `"downloading"`, `"writing"`, `"aggregating"` or
    /// `"done"` — mirrors `senken_loader::Phase`'s own variants.
    pub phase: String,
    /// How many fetch chunks this job's plan requires in total.
    pub chunks_total: u32,
    /// How many of those chunks have been fetched and written so far.
    pub chunks_done: u32,
    /// How many bars have been written so far, across every chunk.
    pub bars_written: u64,
    /// An estimate of the remaining time in seconds — `None` until at least
    /// one chunk has completed.
    pub estimate_secs: Option<f64>,
    /// Set while a chunk fetch is being retried after a transient failure.
    /// Not the same as the job having failed (see
    /// `senken_loader::JobSnapshot::last_error`'s own docs).
    pub last_error: Option<String>,
}

impl From<JobSnapshot> for BarJobDto {
    fn from(snapshot: JobSnapshot) -> Self {
        Self {
            phase: match snapshot.phase {
                Phase::Queued => "queued",
                Phase::Downloading => "downloading",
                Phase::Writing => "writing",
                Phase::Aggregating => "aggregating",
                Phase::Done => "done",
            }
            .to_owned(),
            chunks_total: snapshot.chunks_total,
            chunks_done: snapshot.chunks_done,
            bars_written: snapshot.bars_written,
            estimate_secs: snapshot.estimate.map(|d| d.as_secs_f64()),
            last_error: snapshot.last_error,
        }
    }
}

// ============================================================================
// Indicators.
// ============================================================================

/// A stored `(name, params)` pair, on the wire — mirrors
/// `senken_alerts::IndicatorSpec` field-for-field, reused (not re-declared)
/// for both alerts and indicator computation, the same ten built-ins either
/// way.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub(crate) struct IndicatorSpecDto {
    /// The indicator's name — see `GET /api/indicators`'s catalogue.
    pub name: String,
    /// The indicator's parameters, as JSON-object text.
    pub params: String,
}

impl From<IndicatorSpec> for IndicatorSpecDto {
    fn from(spec: IndicatorSpec) -> Self {
        Self {
            name: spec.name,
            params: spec.params,
        }
    }
}

impl From<IndicatorSpecDto> for IndicatorSpec {
    fn from(dto: IndicatorSpecDto) -> Self {
        Self {
            name: dto.name,
            params: dto.params,
        }
    }
}

/// One entry in `GET /api/indicators`'s catalogue — the ten built-ins
/// `senken-indicators` implements, described well enough that
/// a client can build a settings form without hard-coding this list a
/// second time (closing, one layer up, the exact "second source of truth"
/// the browser's own indicator maths is replaced to avoid).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorCatalogEntry {
    /// The name to pass as `indicator.name` on `POST /api/indicators/compute`
    /// (and, unchanged, as a stored alert's or workspace layer's indicator
    /// name).
    pub name: String,
    /// The JSON object keys `indicator.params` must supply.
    pub params: Vec<String>,
    /// The value keys `POST /api/indicators/compute`'s response reports for
    /// this indicator — see [`IndicatorFieldValue`].
    pub fields: Vec<String>,
}

/// `POST /api/indicators/compute` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ComputeIndicatorRequest {
    /// The instrument, `source:symbol`.
    pub instrument: String,
    /// The bar timeframe, e.g. `"1h"`.
    pub spec: String,
    /// Inclusive start of the range, Unix nanoseconds.
    pub from: i64,
    /// Exclusive end of the range, Unix nanoseconds.
    pub to: i64,
    /// Which indicator to compute, and with what parameters.
    pub indicator: IndicatorSpecDto,
    /// The bar still forming, assembled by the client from live ticks and
    /// therefore not stored anywhere yet.
    ///
    /// Supplied so an indicator's newest point lands on the bar the chart is
    /// actually drawing, instead of stopping one bar behind it. It carries
    /// It carries volume as well as prices: a bar is OHLCV, and an indicator
    /// reading volume is fed from the same stream as one reading price.
    #[serde(default)]
    pub provisional: Option<ProvisionalBarDto>,
}

/// The forming bar a client is drawing, as scaled integers at the
/// instrument's own price scale — the same wire form [`BarDto`] uses.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ProvisionalBarDto {
    /// Start of the forming interval, Unix nanoseconds.
    pub ts_open: i64,
    /// First tick price seen in the interval.
    pub open: i64,
    /// Highest tick price seen so far.
    pub high: i64,
    /// Lowest tick price seen so far.
    pub low: i64,
    /// Most recent tick price.
    pub close: i64,
    /// Base-asset volume accumulated from the ticks seen in this interval,
    /// at the instrument's own quantity scale.
    pub volume: i64,
}

/// One `(field, value)` pair an indicator reports for one bar — see
/// [`IndicatorCatalogEntry::fields`] for which keys a given indicator ever
/// produces (a single-valued indicator reports exactly one, `"value"`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorFieldValue {
    /// The field's name.
    pub field: String,
    /// The field's value at this bar.
    pub value: f64,
}

/// One bar's worth of indicator output, emitted only once the indicator
/// reports `initialized()` ("an EMA's first values are not an EMA" — a warm-up value is never emitted, the same discipline the deleted browser copy honoured).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct IndicatorPointDto {
    /// The bar this point was computed from, Unix nanoseconds.
    pub ts_open: i64,
    /// The indicator's reported value(s) at this bar.
    pub values: Vec<IndicatorFieldValue>,
}

/// `POST /api/indicators/compute` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ComputeIndicatorResponse {
    /// One entry per bar the indicator has initialized for.
    pub points: Vec<IndicatorPointDto>,
    /// Ranges `points` could not cover because the underlying bars are not
    /// resolvable yet — the same contract `BarRangeResponse::missing` makes.
    pub missing: Vec<TimeRangeDto>,
}

// ============================================================================
// Alerts.
// ============================================================================

/// `(field, comparator, threshold)`, on the wire — mirrors
/// `senken_alerts::Condition` field-for-field. `field`/`comparator` reuse
/// that crate's own enums directly (already `Serialize`/`Deserialize`) but
/// are documented to `utoipa` as plain strings via
/// `#[schema(value_type = String)]`, the same technique [`GrantDto`] uses
/// for `senken_acl`'s enums and for the same reason: the orphan rule
/// forbids implementing `ToSchema` for a foreign type from here.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct ConditionDto {
    /// Which of the indicator's own numbers this condition reads.
    #[schema(value_type = String)]
    pub field: IndicatorField,
    /// How the field's value is compared against `threshold`.
    #[schema(value_type = String)]
    pub comparator: Comparator,
    /// The value compared against, in whatever unit `field` already
    /// reports in.
    pub threshold: f64,
}

impl From<Condition> for ConditionDto {
    fn from(condition: Condition) -> Self {
        Self {
            field: condition.field,
            comparator: condition.comparator,
            threshold: condition.threshold,
        }
    }
}

impl From<ConditionDto> for Condition {
    fn from(dto: ConditionDto) -> Self {
        Self {
            field: dto.field,
            comparator: dto.comparator,
            threshold: dto.threshold,
        }
    }
}

/// An alert row ("list, create, delete, and fired-state" — the last three fields plus `enabled` are exactly that state).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AlertDto {
    /// The alert's id.
    pub id: String,
    /// The account that owns this alert.
    pub owner_id: String,
    /// The instrument this alert leases from the subscription pool.
    pub instrument: String,
    /// The bar timeframe this alert's indicator evaluates over.
    pub timeframe: String,
    /// The indicator this alert evaluates.
    pub indicator: IndicatorSpecDto,
    /// The condition checked against the indicator's value each time a bar
    /// closes.
    pub condition: ConditionDto,
    /// Whether this alert is currently being evaluated.
    pub enabled: bool,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to this alert's own fields.
    pub updated_at: i64,
    /// When this alert last fired, if ever.
    pub last_fired_at: Option<i64>,
    /// The indicator value that triggered the most recent fire, if any.
    pub last_fired_value: Option<f64>,
    /// How many times this alert has ever fired.
    pub fire_count: u32,
}

impl From<AlertRecord> for AlertDto {
    fn from(record: AlertRecord) -> Self {
        Self {
            id: record.id.to_string(),
            owner_id: record.owner_id.to_string(),
            instrument: record.instrument.as_str().to_owned(),
            timeframe: record.timeframe.to_string(),
            indicator: record.indicator.into(),
            condition: record.condition.into(),
            enabled: record.enabled,
            created_at: record.created_at,
            updated_at: record.updated_at,
            last_fired_at: record.last_fired_at,
            last_fired_value: record.last_fired_value,
            fire_count: record.fire_count,
        }
    }
}

/// `GET /api/alerts` response body (scope reaches the query, including this `total`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct AlertsPage {
    /// The rows for this page.
    pub rows: Vec<AlertDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// `POST /api/alerts` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateAlertRequest {
    /// The instrument to lease and evaluate, `source:symbol`.
    pub instrument: String,
    /// The bar timeframe to evaluate over, e.g. `"1h"`.
    pub timeframe: String,
    /// The indicator to evaluate.
    pub indicator: IndicatorSpecDto,
    /// The condition to check each time a bar closes.
    pub condition: ConditionDto,
}

/// One instrument-search hit (`GET /api/instruments`). Market data is global
/// and never tenanted, so this carries no owner — every field is drawn from
/// the same cached, ranked catalog `senken-cli`'s own `search`/`instrument`
/// subcommands already read.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct InstrumentSummaryDto {
    /// Fully-qualified id, `source:symbol` — what a caller passes back to
    /// the bars/indicators/alerts endpoints.
    pub id: String,
    /// The source's own id (`id`'s prefix, named separately since a client
    /// group-by-venue without re-parsing `id`).
    pub source_id: String,
    /// The source's display name.
    pub source_name: String,
    /// Normalised symbol (`id`'s suffix).
    pub symbol: String,
    /// Human-readable name (`BTC / USDT`).
    pub name: String,
    /// Base asset code.
    pub base: String,
    /// Quote asset code.
    pub quote: String,
    /// Contract type: `"spot"`, `"perpetual"`, `"future"` or `"option"`.
    pub kind: String,
}

impl From<InstrumentMatch> for InstrumentSummaryDto {
    fn from(hit: InstrumentMatch) -> Self {
        Self {
            id: hit.id.as_str().to_owned(),
            source_id: hit.source_id().to_owned(),
            source_name: hit.source_name.to_string(),
            symbol: hit.instrument.symbol.clone(),
            name: hit.instrument.name.clone(),
            base: hit.instrument.base.clone(),
            quote: hit.instrument.quote.clone(),
            kind: kind_str(hit.instrument.kind).to_owned(),
        }
    }
}

fn kind_str(kind: InstrumentKind) -> &'static str {
    match kind {
        InstrumentKind::Spot => "spot",
        InstrumentKind::Perpetual => "perpetual",
        InstrumentKind::Future => "future",
        InstrumentKind::Option => "option",
        // `InstrumentKind` is `#[non_exhaustive]` — fail closed for a future
        // variant this build does not recognise rather than refuse to
        // compile until every caller everywhere is updated in lockstep.
        _ => "unknown",
    }
}

/// `GET /api/instruments` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct InstrumentsPage {
    /// The hits on this page, best match first.
    pub rows: Vec<InstrumentSummaryDto>,
    /// Hits across all pages, under the same query.
    pub total: u64,
    /// `false` if a source failed to load and is therefore absent from
    /// `rows`/`total` (`senken_marketdata::InstrumentPage::is_complete`) —
    /// surfaced rather than silently dropped, since it changes what
    /// "no match" means.
    pub complete: bool,
}
