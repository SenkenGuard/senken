use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use senken_chart::{
    ChartLayoutDetail, ChartLayoutSummary, ChartWorkspaceSummary, DrawingKind, DrawingLineStyle,
    DrawingPoint, DrawingStyle, ItemSource, PaneItemRecord, PaneRecord, Slot,
};
use senken_indicators::LabelAnchor;

// A text note's label anchor reuses `IndicatorLabelAnchorDto`
// (`dto::indicators`) rather than a second wire enum for the same
// `senken_indicators::LabelAnchor` — see that type's own docs for why.
use super::IndicatorLabelAnchorDto;

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

impl From<ChartWorkspaceSummary> for WorkspaceDto {
    fn from(summary: ChartWorkspaceSummary) -> Self {
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
    /// `senken_chart::LayoutPreset` round-trips through its own
    /// `Display`/`FromStr` (`"1"`, `"2h"`, `"2v"`, `"3h"`, `"3v"`, `"4"`).
    pub preset: String,
    /// This layout's tab order within its workspace.
    pub position: u32,
}

impl From<ChartLayoutSummary> for LayoutSummaryDto {
    fn from(summary: ChartLayoutSummary) -> Self {
        Self {
            id: summary.id.to_string(),
            workspace_id: summary.workspace_id.to_string(),
            name: summary.name,
            preset: summary.preset.to_string(),
            position: summary.position,
        }
    }
}

/// One layer's kind, on the wire. `senken_chart` no longer has a
/// `LayerKind` type of its own (a layer is now a pane item whose
/// `ItemSource` is `Computed`/`Referenced`, placed by `Slot`) — this is
/// this crate's own translation between that domain shape and the wire
/// shape web clients already speak, kept unchanged on purpose. `params`
/// stays an opaque JSON-text string rather than a parsed value, the same
/// way `senken_alerts::IndicatorSpec::params` already does on the wire
/// below: this crate does not interpret it either — that is the indicator
/// engine's job.
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

impl LayerKindDto {
    /// Builds the wire kind for one pane item, or `None` if `source` is an
    /// anchored drawing rather than a layer — the caller only ever calls
    /// this for items already known to be layer-like.
    fn from_item_source(source: &ItemSource, slot: Slot) -> Option<Self> {
        match source {
            ItemSource::Referenced { instrument } => Some(Self::OverlayInstrument {
                instrument: instrument.as_str().to_owned(),
            }),
            ItemSource::Computed { name, params } => Some(match slot {
                Slot::Sub(_) => Self::IndicatorSubPane {
                    name: name.clone(),
                    params: params.clone(),
                },
                Slot::Main => Self::IndicatorOverlay {
                    name: name.clone(),
                    params: params.clone(),
                },
            }),
            ItemSource::Anchored { .. } => None,
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

impl LayerDto {
    /// Builds the wire row for one pane item at `position` (its index
    /// within the layers half of the pane's items, not the item's own
    /// stored global position — see `PaneDto::from`), or `None` if `item`
    /// is an anchored drawing.
    fn from_item(item: &PaneItemRecord, position: u32) -> Option<Self> {
        Some(Self {
            id: item.id.to_string(),
            position,
            kind: LayerKindDto::from_item_source(&item.source, item.slot)?,
            visible: item.visible,
            style: item.style.clone(),
        })
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

/// One (time, price) anchor for a multi-point [`DrawingKindDto`], on the
/// wire.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, ToSchema)]
pub(crate) struct DrawingPointDto {
    /// Unix nanoseconds, matching every other time value this crate puts on
    /// the wire (`super::BarDto`'s `ts_open`, `TimeRangeDto`'s own fields).
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

impl From<DrawingPointDto> for DrawingPoint {
    fn from(point: DrawingPointDto) -> Self {
        Self {
            time: senken_core::UnixNanos::from_nanos(point.time),
            price: point.price,
        }
    }
}

fn label_anchor_from_domain(anchor: LabelAnchor) -> IndicatorLabelAnchorDto {
    match anchor {
        LabelAnchor::Above => IndicatorLabelAnchorDto::Above,
        LabelAnchor::Below => IndicatorLabelAnchorDto::Below,
        LabelAnchor::Center => IndicatorLabelAnchorDto::Center,
    }
}

fn label_anchor_to_domain(anchor: IndicatorLabelAnchorDto) -> LabelAnchor {
    match anchor {
        IndicatorLabelAnchorDto::Above => LabelAnchor::Above,
        IndicatorLabelAnchorDto::Below => LabelAnchor::Below,
        IndicatorLabelAnchorDto::Center => LabelAnchor::Center,
    }
}

/// One drawing's kind and geometry, on the wire — mirrors
/// `senken_chart::DrawingKind` field-for-field. `Ray`/`FibRetracement`/
/// `TextNote` are additions: the three tools that had no schema to persist
/// into before this build (015 maps them onto `Segment { extend: Forward
/// }` / `Level × 6` / `Label` respectively; this crate only carries their
/// geometry, never renders it).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
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
    /// A straight line through two points, extended forever past the
    /// second one.
    Ray {
        /// The ray's origin.
        start: DrawingPointDto,
        /// The ray's second anchor; the ray extends past this point.
        end: DrawingPointDto,
    },
    /// A Fibonacci retracement between a high and a low.
    FibRetracement {
        /// One end of the retracement range.
        start: DrawingPointDto,
        /// The other end.
        end: DrawingPointDto,
    },
    /// Free text anchored to one chart point.
    TextNote {
        /// Where the note is anchored.
        at: DrawingPointDto,
        /// The note's text.
        text: String,
        /// The label's position relative to `at`.
        anchor: IndicatorLabelAnchorDto,
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
            DrawingKind::Ray { start, end } => Self::Ray {
                start: start.into(),
                end: end.into(),
            },
            DrawingKind::FibRetracement { start, end } => Self::FibRetracement {
                start: start.into(),
                end: end.into(),
            },
            DrawingKind::TextNote { at, text, anchor } => Self::TextNote {
                at: at.into(),
                text,
                anchor: label_anchor_from_domain(anchor),
            },
        }
    }
}

impl From<DrawingKindDto> for DrawingKind {
    fn from(dto: DrawingKindDto) -> Self {
        match dto {
            DrawingKindDto::HorizontalLine { price } => Self::HorizontalLine { price },
            DrawingKindDto::TrendLine { start, end } => Self::TrendLine {
                start: start.into(),
                end: end.into(),
            },
            DrawingKindDto::Rectangle { start, end } => Self::Rectangle {
                start: start.into(),
                end: end.into(),
            },
            DrawingKindDto::Ray { start, end } => Self::Ray {
                start: start.into(),
                end: end.into(),
            },
            DrawingKindDto::FibRetracement { start, end } => Self::FibRetracement {
                start: start.into(),
                end: end.into(),
            },
            DrawingKindDto::TextNote { at, text, anchor } => Self::TextNote {
                at: at.into(),
                text,
                anchor: label_anchor_to_domain(anchor),
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

impl From<DrawingLineStyleDto> for DrawingLineStyle {
    fn from(style: DrawingLineStyleDto) -> Self {
        match style {
            DrawingLineStyleDto::Solid => Self::Solid,
            DrawingLineStyleDto::Dashed => Self::Dashed,
            DrawingLineStyleDto::Dotted => Self::Dotted,
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
    /// Whether the drawing is currently shown — new in schema v8; a
    /// drawing could not express this at all before.
    pub visible: bool,
    /// The drawing's colour, as `#rrggbb`.
    pub color: String,
    /// Stroke width in pixels, 1 through 4.
    pub width: u8,
    /// The drawing's line style.
    pub line_style: DrawingLineStyleDto,
}

impl DrawingDto {
    /// Builds the wire row for one pane item at `position` (its index
    /// within the drawings half of the pane's items — see
    /// `PaneDto::from`), or `None` if `item` is not an anchored drawing.
    /// A style that fails to decode (which write-time validation should
    /// already have made impossible) falls back to a visible default
    /// rather than making this conversion — and therefore `GET
    /// /api/layouts/{id}` — fallible over one corrupt row.
    fn from_item(item: &PaneItemRecord, position: u32) -> Option<Self> {
        let ItemSource::Anchored { kind } = &item.source else {
            return None;
        };
        let style = DrawingStyle::from_json(&item.style).unwrap_or_else(|error| {
            tracing::error!(
                item_id = %item.id,
                %error,
                "chart store: pane item style failed to decode; using a fallback style"
            );
            DrawingStyle {
                color: "#808080".to_owned(),
                width: 1,
                line_style: DrawingLineStyle::Solid,
            }
        });
        Some(Self {
            id: item.id.to_string(),
            position,
            kind: kind.clone().into(),
            visible: item.visible,
            color: style.color,
            width: style.width,
            line_style: style.line_style.into(),
        })
    }
}

/// One drawing to write, supplied as part of a [`PaneInputDto`].
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct DrawingInputDto {
    /// This drawing's stacking order within its pane.
    pub position: u32,
    /// What kind of drawing this is, and its geometry.
    pub kind: DrawingKindDto,
    /// Whether the drawing starts out visible. Defaults to `true` — a
    /// request from before this field existed carries no opinion, and
    /// every drawing rendered unconditionally until now.
    #[serde(default = "default_true")]
    pub visible: bool,
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
        // `senken_chart` now stores every item — computed indicator,
        // referenced overlay instrument, or anchored drawing — in one
        // table and one stacking order. The wire shape still splits them
        // into `layers`/`drawings`, unchanged since before that merge, so
        // the split (and the renumbering that goes with it) happens right
        // here rather than in the domain crate.
        let mut layers = Vec::new();
        let mut drawings = Vec::new();
        for item in &record.items {
            if let ItemSource::Anchored { .. } = &item.source {
                let position = u32::try_from(drawings.len()).unwrap_or(u32::MAX);
                if let Some(dto) = DrawingDto::from_item(item, position) {
                    drawings.push(dto);
                }
            } else {
                let position = u32::try_from(layers.len()).unwrap_or(u32::MAX);
                if let Some(dto) = LayerDto::from_item(item, position) {
                    layers.push(dto);
                }
            }
        }
        Self {
            id: record.id.to_string(),
            position: record.position,
            instrument: record.instrument.as_str().to_owned(),
            timeframe: record.timeframe.to_string(),
            layers,
            drawings,
            settings: record.settings,
        }
    }
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

fn default_true() -> bool {
    true
}

/// A layout together with its panes and their items — the full nested
/// structure workspace → layout → panes → items, minus the workspace
/// itself.
#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct LayoutDetailDto {
    /// The layout's identity and grid arrangement.
    pub layout: LayoutSummaryDto,
    /// The layout's panes, in grid-slot order.
    pub panes: Vec<PaneDto>,
}

impl From<ChartLayoutDetail> for LayoutDetailDto {
    fn from(detail: ChartLayoutDetail) -> Self {
        Self {
            layout: detail.layout.into(),
            panes: detail.panes.into_iter().map(PaneDto::from).collect(),
        }
    }
}

/// `PUT /api/layouts/{id}` request body: replaces a layout's entire pane/
/// item structure in one transaction.
#[derive(Debug, Deserialize, ToSchema)]
pub(crate) struct ReplaceLayoutRequest {
    /// The pane-grid arrangement, one of `senken_chart::LayoutPreset`'s
    /// own tokens (`"1"`, `"2h"`, `"2v"`, `"3h"`, `"3v"`, `"4"`).
    pub preset: String,
    /// The layout's new panes. Length must match `preset`'s pane count.
    pub panes: Vec<PaneInputDto>,
}
