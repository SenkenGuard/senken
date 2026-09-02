use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_dashboard::{
    DashboardLayout, DashboardWorkspaceSummary, DataSource, GridSize, WidgetDefinition,
    WidgetRecord,
};

/// A dashboard workspace row (list/create/rename/delete).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DashboardWorkspaceDto {
    /// The workspace's id.
    pub id: String,
    /// The account that owns this workspace.
    pub owner_id: String,
    /// The workspace's display name.
    pub name: String,
    /// How many grid columns this workspace's layout uses.
    pub columns: u32,
    /// The optimistic-concurrency token a client must echo back as
    /// `expected_revision` when it next calls `PUT .../layout`.
    pub revision: u64,
    /// Unix timestamp of creation.
    pub created_at: i64,
    /// Unix timestamp of the last change to the workspace's own fields or
    /// its widget layout.
    pub updated_at: i64,
}

impl From<DashboardWorkspaceSummary> for DashboardWorkspaceDto {
    fn from(summary: DashboardWorkspaceSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            owner_id: summary.owner_id.to_string(),
            name: summary.name,
            columns: summary.columns,
            revision: summary.revision,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
        }
    }
}

/// `GET /api/dashboard/workspaces` response body (scope reaches the query,
/// including this `total`).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DashboardWorkspacesPage {
    /// The rows for this page.
    pub rows: Vec<DashboardWorkspaceDto>,
    /// How many rows exist in total, under the same scope as `rows`.
    pub total: u64,
}

/// `POST /api/dashboard/workspaces` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct CreateDashboardWorkspaceRequest {
    /// The new workspace's display name.
    pub name: String,
}

/// `GET /api/dashboard/workspaces/default` response body ("opening the
/// dashboard with no workspace creates a default one" belongs on the
/// server, not in the client, so it holds for any client that ever
/// connects).
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DefaultDashboardWorkspaceResponse {
    /// The caller's default workspace, created on first call and returned
    /// unchanged on every later one.
    pub workspace_id: String,
}

/// `PATCH /api/dashboard/workspaces/{id}` request body.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct RenameDashboardWorkspaceRequest {
    /// The workspace's new display name.
    pub name: String,
}

/// One widget placed on a workspace's grid, on the wire.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DashboardWidgetDto {
    /// This widget instance's id.
    pub id: String,
    /// The plugin (or `"senken"` for a built-in) that contributes this
    /// widget's rendering.
    pub provider_id: String,
    /// `<provider_id>/<widget>` — looked up against `GET
    /// /api/dashboard/widgets/catalog` to render this widget for real, or
    /// a placeholder when the lookup misses.
    pub widget_type_id: String,
    /// This widget's left edge, in grid columns — never pixels.
    pub position_x: u32,
    /// This widget's top edge, in grid rows.
    pub position_y: u32,
    /// This widget's width, in grid columns.
    pub width: u32,
    /// This widget's height, in grid rows.
    pub height: u32,
    /// Whether this widget is currently shown.
    pub visible: bool,
    /// This widget's configuration, as opaque JSON-object text. The
    /// server stores and returns it without interpreting it; the widget's
    /// own provider owns what the fields inside mean.
    pub config: String,
    /// The version of `config`'s own shape.
    pub config_schema_version: u32,
    /// Unix timestamp this widget was first placed.
    pub created_at: i64,
    /// Unix timestamp of the last change to this widget's placement,
    /// visibility or config.
    pub updated_at: i64,
}

impl From<WidgetRecord> for DashboardWidgetDto {
    fn from(record: WidgetRecord) -> Self {
        Self {
            id: record.id.to_string(),
            provider_id: record.provider_id,
            widget_type_id: record.widget_type_id,
            position_x: record.position_x,
            position_y: record.position_y,
            width: record.width,
            height: record.height,
            visible: record.visible,
            config: record.config,
            config_schema_version: record.config_schema_version,
            created_at: record.created_at,
            updated_at: record.updated_at,
        }
    }
}

/// `GET /api/dashboard/workspaces/{id}/layout` response body.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DashboardLayoutDto {
    /// The workspace's own identity and grid width.
    pub workspace: DashboardWorkspaceDto,
    /// Every widget placed on this workspace's grid.
    pub widgets: Vec<DashboardWidgetDto>,
}

impl From<DashboardLayout> for DashboardLayoutDto {
    fn from(layout: DashboardLayout) -> Self {
        Self {
            workspace: layout.workspace.into(),
            widgets: layout.widgets.into_iter().map(Into::into).collect(),
        }
    }
}

/// One widget to write, as part of `PUT
/// /api/dashboard/workspaces/{id}/layout`'s full-workspace snapshot.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct DashboardWidgetPlacementRequest {
    /// Omitted for a newly added widget (a fresh id is generated by the
    /// server); present to update an existing widget in place, preserving
    /// its id. An id that does not already belong to this workspace is
    /// rejected with `400`, not silently adopted.
    #[serde(default)]
    pub id: Option<String>,
    /// The rendering provider.
    pub provider_id: String,
    /// The widget type, `<provider_id>/<widget>`.
    pub widget_type_id: String,
    /// This widget's left edge, in grid columns.
    pub position_x: u32,
    /// This widget's top edge, in grid rows.
    pub position_y: u32,
    /// This widget's width, in grid columns.
    pub width: u32,
    /// This widget's height, in grid rows.
    pub height: u32,
    /// Whether this widget is currently shown.
    pub visible: bool,
    /// This widget's configuration, as opaque JSON-object text.
    pub config: String,
    /// The version of `config`'s own shape.
    pub config_schema_version: u32,
}

/// `PUT /api/dashboard/workspaces/{id}/layout` request body. Every widget
/// the caller wants kept — moved, resized or entirely unchanged — must be
/// present in `widgets` with its own id; any existing widget whose id is
/// not present is deleted. This is the one call add/move/resize/delete all
/// go through, guarded by `expected_revision`.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ReplaceDashboardLayoutRequest {
    /// The workspace's `revision` this snapshot was built against. A
    /// mismatch against the server's current revision means another write
    /// (most likely a second open tab) landed first, and this call is
    /// refused with `409` rather than silently overwriting it.
    pub expected_revision: u64,
    /// How many grid columns this workspace's layout uses.
    pub columns: u32,
    /// The workspace's complete widget grid after this call.
    pub widgets: Vec<DashboardWidgetPlacementRequest>,
}

// `PUT /api/dashboard/workspaces/{id}/layout` responds with a
// `DashboardLayoutDto`, the same full shape `GET .../layout` returns —
// not a bare `{ revision }`. A newly added widget is given a fresh id by
// the server, and that id never appears anywhere in the request; the
// caller's only way to learn it (so a *later* move or resize can name it
// with `id: Some(..)` instead of creating yet another row) is to read it
// back from this same response, in the same round trip.

/// Where a widget's data comes from, on the wire — see
/// `senken_dashboard::DataSource`'s own docs.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DashboardDataSourceDto {
    /// Reads real, live data.
    Live,
    /// Renders a fixed, seeded example rather than anything real. The
    /// dashboard grid draws a mockup label over any widget that reports
    /// this.
    Mock,
}

impl From<DataSource> for DashboardDataSourceDto {
    fn from(data_source: DataSource) -> Self {
        match data_source {
            DataSource::Live => Self::Live,
            DataSource::Mock => Self::Mock,
        }
    }
}

/// A grid size, in grid cells — never pixels.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DashboardGridSizeDto {
    /// Width, in grid columns.
    pub width: u32,
    /// Height, in grid rows.
    pub height: u32,
}

impl From<GridSize> for DashboardGridSizeDto {
    fn from(size: GridSize) -> Self {
        Self {
            width: size.width,
            height: size.height,
        }
    }
}

/// One widget type's metadata, on the wire — never an instance.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DashboardWidgetDefinitionDto {
    /// `<provider_id>/<widget>`.
    pub widget_type_id: String,
    /// The provider that contributes this widget.
    pub provider_id: String,
    /// Display title, for the "add widget" picker and the widget's own
    /// header.
    pub title: String,
    /// A one-line description, for the "add widget" picker.
    pub description: String,
    /// The size a newly added instance starts at.
    pub default_size: DashboardGridSizeDto,
    /// The smallest size this widget can be resized to.
    pub min_size: DashboardGridSizeDto,
    /// Where this widget's data comes from.
    pub data_source: DashboardDataSourceDto,
}

impl From<&WidgetDefinition> for DashboardWidgetDefinitionDto {
    fn from(definition: &WidgetDefinition) -> Self {
        Self {
            widget_type_id: definition.widget_type_id.clone(),
            provider_id: definition.provider_id.clone(),
            title: definition.title.clone(),
            description: definition.description.clone(),
            default_size: definition.default_size.into(),
            min_size: definition.min_size.into(),
            data_source: definition.data_source.into(),
        }
    }
}

/// `GET /api/dashboard/widgets/catalog` response body — every widget type
/// this build's server currently knows how to serve.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct DashboardWidgetCatalogResponse {
    /// The effective catalog.
    pub widgets: Vec<DashboardWidgetDefinitionDto>,
}
