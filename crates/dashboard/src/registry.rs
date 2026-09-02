//! The widget registry: every widget type this build's server knows how to
//! serve, and the "effective catalog" a workspace's grid is checked
//! against.
//!
//! A placed widget ([`crate::WidgetPlacement`]) stores a definition's id and
//! a user's own config, never a copy of the definition itself and never a
//! component — see this crate's module docs for why that is what makes a
//! placeholder possible.

use std::collections::HashMap;

/// The provider id every built-in widget in this registry uses. Reserved:
/// no plugin may register a widget under this id.
pub const BUILTIN_PROVIDER_ID: &str = "senken";

/// Where a widget's data comes from. The host uses this to decide whether
/// to draw a mockup label over a placed instance of this widget — drawn by
/// the host, never the widget itself, so a widget cannot suppress its own
/// label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    /// Reads real, live data.
    Live,
    /// Renders a fixed, seeded example rather than anything real.
    Mock,
}

/// A grid size, in grid cells. Never pixels — see this crate's module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    /// Width, in grid columns.
    pub width: u32,
    /// Height, in grid rows.
    pub height: u32,
}

/// One widget type's metadata — a definition, never an instance. Metadata
/// here is not copied onto a placed widget's row; a workspace stores only
/// [`WidgetDefinition::widget_type_id`] and the user's own config.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WidgetDefinition {
    /// `<provider_id>/<widget>` — this build's built-ins all use
    /// [`BUILTIN_PROVIDER_ID`].
    pub widget_type_id: String,
    /// The provider that contributes this widget.
    pub provider_id: String,
    /// Display title, for the "add widget" picker and the widget's own
    /// header.
    pub title: String,
    /// A one-line description, for the "add widget" picker.
    pub description: String,
    /// The size a newly added instance starts at.
    pub default_size: GridSize,
    /// The smallest size this widget can be resized to.
    pub min_size: GridSize,
    /// Where this widget's data comes from.
    pub data_source: DataSource,
}

fn builtin_definition(
    widget: &str,
    title: &str,
    description: &str,
    default_width: u32,
    default_height: u32,
) -> WidgetDefinition {
    WidgetDefinition {
        widget_type_id: format!("{BUILTIN_PROVIDER_ID}/{widget}"),
        provider_id: BUILTIN_PROVIDER_ID.to_owned(),
        title: title.to_owned(),
        description: description.to_owned(),
        default_size: GridSize {
            width: default_width,
            height: default_height,
        },
        // Every built-in shrinks to the narrowest column count this grid
        // offers and a two-row minimum — small enough to still show a
        // header and one line of content.
        min_size: GridSize {
            width: 3,
            height: 2,
        },
        // Every widget this build ships renders a seeded fixture: an
        // equity curve needs a funded, connected broker account to show a
        // real balance, and none is wired up yet.
        data_source: DataSource::Mock,
    }
}

/// The widget catalog: every widget type this build's server currently
/// knows how to serve, keyed by `widget_type_id`.
///
/// Nothing here is persisted — [`crate::DashboardWorkspaceStore`] stores a
/// placed widget's `widget_type_id` as plain text with no foreign key into
/// this registry, because a registry entry can disappear (a plugin
/// disabled, or simply an older build) while a stored layout must still
/// read back. A caller cross-references a stored widget's `widget_type_id`
/// against [`WidgetRegistry::get`] to decide whether that widget is still
/// available or should render as a placeholder.
#[derive(Debug, Clone)]
pub struct WidgetRegistry {
    definitions: HashMap<String, WidgetDefinition>,
    order: Vec<String>,
}

impl WidgetRegistry {
    /// The registry of every widget type this build ships without a
    /// plugin. Replaces the old conditional (`if widget.type == "equity"
    /// { .. } else if ..`) render path with a lookup by id, the same shape
    /// [`WidgetRegistry::get`] gives every future plugin-contributed widget
    /// too.
    #[must_use]
    pub fn builtin() -> Self {
        let definitions = [
            builtin_definition("equity", "Equity Curve", "balance + benchmark", 6, 4),
            builtin_definition("watchlist", "Watchlist", "tracked pairs", 3, 4),
            builtin_definition("positions", "Open Positions", "live PnL table", 6, 4),
            builtin_definition("risk", "Risk Meters", "exposure gauges", 3, 3),
            builtin_definition("heatmap", "Volatility Heatmap", "daily grid", 4, 3),
            builtin_definition("signals", "Signal Desk", "model verdicts", 4, 3),
            builtin_definition("flow", "Buy / Sell Flow", "volume split", 4, 3),
            builtin_definition("feed", "News Tape", "headline stream", 4, 3),
        ];
        let order = definitions
            .iter()
            .map(|d| d.widget_type_id.clone())
            .collect();
        let definitions = definitions
            .into_iter()
            .map(|d| (d.widget_type_id.clone(), d))
            .collect();
        Self { definitions, order }
    }

    /// The widget definitions in this registry, in stable declaration
    /// order — for a `GET`-style catalog listing, which must not reorder
    /// itself between two calls.
    #[must_use]
    pub fn catalog(&self) -> Vec<&WidgetDefinition> {
        self.order
            .iter()
            .filter_map(|id| self.definitions.get(id))
            .collect()
    }

    /// Looks up one widget type by id.
    #[must_use]
    pub fn get(&self, widget_type_id: &str) -> Option<&WidgetDefinition> {
        self.definitions.get(widget_type_id)
    }

    /// `true` if `widget_type_id` is in this registry's effective catalog —
    /// the check that decides whether a placed widget renders for real or
    /// as a placeholder.
    #[must_use]
    pub fn contains(&self, widget_type_id: &str) -> bool {
        self.definitions.contains_key(widget_type_id)
    }
}

impl Default for WidgetRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

#[cfg(test)]
mod tests {
    use super::{BUILTIN_PROVIDER_ID, DataSource, WidgetRegistry};

    #[test]
    fn every_builtin_widget_is_found_by_its_own_id() {
        let registry = WidgetRegistry::builtin();
        for definition in registry.catalog() {
            assert!(registry.contains(&definition.widget_type_id));
            assert_eq!(registry.get(&definition.widget_type_id), Some(definition));
        }
    }

    #[test]
    fn an_unknown_widget_type_is_not_found() {
        let registry = WidgetRegistry::builtin();
        assert!(!registry.contains("senken/does-not-exist"));
        assert!(!registry.contains("some-plugin/equity"));
        assert_eq!(registry.get("senken/does-not-exist"), None);
    }

    #[test]
    fn every_builtin_widget_reports_mock_data() {
        // No built-in widget has a broker account to read real numbers
        // from yet; every one of them must say so rather than let the host
        // guess.
        let registry = WidgetRegistry::builtin();
        for definition in registry.catalog() {
            assert_eq!(
                definition.data_source,
                DataSource::Mock,
                "{} must report DataSource::Mock",
                definition.widget_type_id
            );
            assert_eq!(definition.provider_id, BUILTIN_PROVIDER_ID);
        }
    }

    #[test]
    fn the_catalog_lists_all_eight_builtin_widgets_in_a_stable_order() {
        let registry = WidgetRegistry::builtin();
        let ids: Vec<&str> = registry
            .catalog()
            .iter()
            .map(|d| d.widget_type_id.as_str())
            .collect();
        assert_eq!(
            ids,
            vec![
                "senken/equity",
                "senken/watchlist",
                "senken/positions",
                "senken/risk",
                "senken/heatmap",
                "senken/signals",
                "senken/flow",
                "senken/feed",
            ]
        );
        // Calling again must produce the exact same order, not one drawn
        // from `HashMap` iteration order.
        assert_eq!(
            ids,
            registry
                .catalog()
                .iter()
                .map(|d| d.widget_type_id.as_str())
                .collect::<Vec<_>>()
        );
    }
}
