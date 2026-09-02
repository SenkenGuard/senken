//! The manifest a dynamic widget UI package declares, and its validation.
//!
//! A package is a directory containing `manifest.json` and a `web/`
//! directory of static assets (`index.html`, scripts, styles) — no source,
//! nothing the host ever compiles or executes outside a sandboxed iframe.
//! [`validate()`](crate::widget_package::manifest::validate) is the one
//! function that turns the untrusted bytes of a `manifest.json` into a
//! [`ValidatedManifest`] the rest of this crate trusts; nothing downstream
//! reads the raw, unvalidated manifest struct again.
//!
//! # Extension points are named, not imperative
//!
//! A manifest declares which named point it contributes to — today only
//! `dashboard.widget` is wired to a host renderer. Four more names
//! (`chart.toolbar.item`, `statusbar.item`, `topbar.item`,
//! `settings.section`) are agreed in shape but not yet routed anywhere, and
//! any other name is not agreed at all. `validate()` rejects both cases
//! loudly, naming the point, rather than accepting and silently ignoring
//! it — this project has already shipped five features that failed with no
//! signal at all (a selector never emitted, a color never exported, a
//! border with no width), and a plugin author has no other way to find out
//! their contribution did nothing.
//!
//! # What a manifest is never trusted to say
//!
//! A widget's `widget_type_id` (`<provider_id>/<widget id>`) is always
//! derived from the package's own id plus the widget's own short id inside
//! it, never read from a `providerId` field in the manifest itself — a
//! package cannot claim to be contributing on another provider's behalf,
//! by mistake or otherwise.

use std::collections::HashSet;
use std::path::{Component, Path};

use senken_acl::PluginNamespace;
use serde::Deserialize;

/// The one widget contribution `apiVersion` this build understands.
const WIDGET_API_VERSION: &str = "senken.widget/v1";

/// The extension point wired to a real host renderer today. See this
/// module's docs.
pub const DASHBOARD_WIDGET_POINT: &str = "dashboard.widget";

/// Extension point names this document has agreed the *shape* of, but whose
/// host-side renderer does not exist yet. Declaring one of these must fail
/// loudly, naming exactly that point, rather than being accepted and then
/// never doing anything.
const RESERVED_NOT_YET_ROUTED_POINTS: &[&str] = &[
    "chart.toolbar.item",
    "statusbar.item",
    "topbar.item",
    "settings.section",
];

/// Everything that can be wrong with an uploaded or discovered package's
/// `manifest.json`. Every variant names the offending value, so a plugin
/// author gets a message that tells them what to fix, not just that
/// something failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ManifestError {
    /// The bytes are not valid JSON, or not shaped like a manifest at all.
    #[error("manifest.json is not valid: {0}")]
    Malformed(String),
    /// `id` is not a valid plugin identifier.
    #[error("plugin id {0:?} is not a valid identifier (lowercase letters, digits and '-')")]
    InvalidProviderId(String),
    /// `contributes` was empty — a package that contributes nothing is not
    /// a package.
    #[error("a package must declare at least one contribution")]
    NoContributions,
    /// `point` is not a name this build has agreed the shape of at all.
    #[error("extension point {0:?} is not a recognized name")]
    UnknownExtensionPoint(String),
    /// `point` is a name this build has agreed the shape of, but no host
    /// renderer exists for it yet.
    #[error("extension point {0:?} has no host renderer yet and cannot be used")]
    ExtensionPointNotYetAvailable(String),
    /// A `dashboard.widget` contribution had no `widget` object.
    #[error("a {DASHBOARD_WIDGET_POINT} contribution is missing its \"widget\" definition")]
    MissingWidgetDefinition,
    /// A widget's own short `id` is not a valid identifier.
    #[error("widget id {0:?} is not a valid identifier (lowercase letters, digits and '-')")]
    InvalidWidgetId(String),
    /// Two widgets in the same package declared the same `id`.
    #[error("widget id {0:?} is declared more than once in this package")]
    DuplicateWidgetId(String),
    /// The widget declared an `apiVersion` this build does not understand.
    #[error(
        "widget {0:?} declares apiVersion {1:?}; this build only understands \"senken.widget/v1\""
    )]
    UnsupportedApiVersion(String, String),
    /// The widget's `title` was empty.
    #[error("widget {0:?} has an empty title")]
    EmptyTitle(String),
    /// `defaultSize` or `minSize` had a zero width or height.
    #[error("widget {0:?} declares a zero-sized {1}")]
    ZeroSize(String, &'static str),
    /// `minSize` was larger than `defaultSize` on some axis.
    #[error("widget {0:?} declares a minSize larger than its own defaultSize")]
    MinSizeExceedsDefault(String),
    /// `maxSize` was smaller than `defaultSize` on some axis.
    #[error("widget {0:?} declares a maxSize smaller than its own defaultSize")]
    MaxSizeBelowDefault(String),
    /// `dataSource` was not `"mock"` or `"live"`.
    #[error("widget {0:?} declares dataSource {1:?}; must be \"mock\" or \"live\"")]
    InvalidDataSource(String, String),
    /// `entry` was empty.
    #[error("widget {0:?} has an empty entry path")]
    EmptyEntry(String),
    /// `entry` was absolute or attempted to escape the package (`..`).
    #[error(
        "widget {0:?}'s entry path {1:?} must be relative and cannot escape the package (no leading \"/\", no \"..\")"
    )]
    UnsafeEntryPath(String, String),
    /// `configSchema` was present but not a JSON object.
    #[error("widget {0:?}'s configSchema must be a JSON object")]
    InvalidConfigSchema(String),
}

#[derive(Debug, Deserialize)]
struct RawPackageManifest {
    id: String,
    name: String,
    version: String,
    #[serde(default)]
    description: String,
    contributes: Vec<RawContribution>,
}

#[derive(Debug, Deserialize)]
struct RawContribution {
    point: String,
    #[serde(default)]
    widget: Option<RawWidgetContribution>,
}

#[derive(Debug, Deserialize)]
struct RawGridSize {
    width: u32,
    height: u32,
}

#[derive(Debug, Deserialize)]
struct RawWidgetContribution {
    #[serde(rename = "apiVersion")]
    api_version: String,
    id: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    category: String,
    #[serde(rename = "defaultSize")]
    default_size: RawGridSize,
    #[serde(rename = "minSize")]
    min_size: RawGridSize,
    #[serde(rename = "maxSize", default)]
    max_size: Option<RawGridSize>,
    #[serde(rename = "configSchemaVersion", default = "default_schema_version")]
    config_schema_version: u32,
    #[serde(rename = "configSchema", default)]
    config_schema: serde_json::Value,
    #[serde(rename = "requiredPermissions", default)]
    required_permissions: Vec<String>,
    #[serde(rename = "requiredCapabilities", default)]
    required_capabilities: Vec<String>,
    #[serde(rename = "dataSource")]
    data_source: String,
    entry: String,
}

fn default_schema_version() -> u32 {
    1
}

/// A grid size, in grid cells — never pixels, matching every other geometry
/// value this platform stores (`senken_dashboard`'s own module docs give the
/// full reasoning).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridSize {
    /// Width, in grid columns.
    pub width: u32,
    /// Height, in grid rows.
    pub height: u32,
}

/// Where a widget's data comes from — drives whether the host draws a
/// mockup label over it. See this crate's `widget_package` module docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataSource {
    /// Reads real, live data.
    Live,
    /// Renders a fixed or synthetic example rather than anything real.
    Mock,
}

/// One validated `dashboard.widget` contribution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedWidgetContribution {
    /// `<provider_id>/<widget id>` — derived, never read from the manifest
    /// as a whole value (see this module's docs).
    pub widget_type_id: String,
    /// The widget's own short id within its package.
    pub widget_id: String,
    /// Display title.
    pub title: String,
    /// A one-line description.
    pub description: String,
    /// A free-text grouping label for the "add widget" picker.
    pub category: String,
    /// The size a newly added instance starts at.
    pub default_size: GridSize,
    /// The smallest size this widget may be resized to.
    pub min_size: GridSize,
    /// The largest size this widget may be resized to, if bounded.
    pub max_size: Option<GridSize>,
    /// The version of this widget's own `config` shape.
    pub config_schema_version: u32,
    /// A JSON-object schema for this widget's `config`, provided as-is; the
    /// host never interprets its fields.
    pub config_schema: serde_json::Value,
    /// Permission names this widget needs granted before it renders for
    /// real (not yet enforced — carried through for a future gate).
    pub required_permissions: Vec<String>,
    /// Host capability names this widget needs (not yet enforced — carried
    /// through for a future gate).
    pub required_capabilities: Vec<String>,
    /// Whether this widget reads real data or renders a fixed/synthetic
    /// example — see [`DataSource`].
    pub data_source: DataSource,
    /// Path, relative to the package's own `web/` directory, to the
    /// bundle's entry document (e.g. `index.html`). Already checked to be
    /// relative and non-escaping; [`crate::widget_package::store`]
    /// re-checks this at asset-resolution time too, as defense in depth.
    pub entry: String,
}

/// A validated widget UI package manifest. Nothing downstream ever reads
/// the raw JSON again once this exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedManifest {
    /// The package's own id — also every contributed widget's provider id.
    pub provider_id: String,
    /// Display name.
    pub name: String,
    /// The package's own version string.
    pub version: String,
    /// A one-line description.
    pub description: String,
    /// Every `dashboard.widget` contribution this package declares.
    pub widgets: Vec<ValidatedWidgetContribution>,
}

/// Validates `raw` (the bytes of a `manifest.json`) into a
/// [`ValidatedManifest`].
///
/// # Errors
/// See [`ManifestError`]'s variants — every one names the specific value
/// that failed, rather than a generic "invalid manifest".
pub fn validate(raw: &[u8]) -> Result<ValidatedManifest, ManifestError> {
    let parsed: RawPackageManifest =
        serde_json::from_slice(raw).map_err(|e| ManifestError::Malformed(e.to_string()))?;

    let provider_id = PluginNamespace::new(&parsed.id)
        .map_err(|_| ManifestError::InvalidProviderId(parsed.id.clone()))?;

    if parsed.contributes.is_empty() {
        return Err(ManifestError::NoContributions);
    }

    let mut widgets = Vec::with_capacity(parsed.contributes.len());
    let mut seen_widget_ids = HashSet::new();
    for contribution in parsed.contributes {
        match contribution.point.as_str() {
            DASHBOARD_WIDGET_POINT => {
                let raw_widget = contribution
                    .widget
                    .ok_or(ManifestError::MissingWidgetDefinition)?;
                let widget = validate_widget(provider_id.id(), raw_widget)?;
                if !seen_widget_ids.insert(widget.widget_id.clone()) {
                    return Err(ManifestError::DuplicateWidgetId(widget.widget_id));
                }
                widgets.push(widget);
            }
            point if RESERVED_NOT_YET_ROUTED_POINTS.contains(&point) => {
                return Err(ManifestError::ExtensionPointNotYetAvailable(
                    point.to_owned(),
                ));
            }
            other => return Err(ManifestError::UnknownExtensionPoint(other.to_owned())),
        }
    }

    Ok(ValidatedManifest {
        provider_id: provider_id.id().to_owned(),
        name: parsed.name,
        version: parsed.version,
        description: parsed.description,
        widgets,
    })
}

fn validate_widget(
    provider_id: &str,
    raw: RawWidgetContribution,
) -> Result<ValidatedWidgetContribution, ManifestError> {
    if raw.api_version != WIDGET_API_VERSION {
        return Err(ManifestError::UnsupportedApiVersion(
            raw.id,
            raw.api_version,
        ));
    }
    // The widget's own short id is validated with the same slug rule as a
    // plugin id: it becomes the second half of `widget_type_id`, which is
    // itself parsed the same way a permission name's resource segment is.
    if PluginNamespace::new(&raw.id).is_err() {
        return Err(ManifestError::InvalidWidgetId(raw.id));
    }
    if raw.title.trim().is_empty() {
        return Err(ManifestError::EmptyTitle(raw.id));
    }
    let default_size = to_grid_size(&raw.default_size);
    let min_size = to_grid_size(&raw.min_size);
    if default_size.width == 0 || default_size.height == 0 {
        return Err(ManifestError::ZeroSize(raw.id, "defaultSize"));
    }
    if min_size.width == 0 || min_size.height == 0 {
        return Err(ManifestError::ZeroSize(raw.id, "minSize"));
    }
    if min_size.width > default_size.width || min_size.height > default_size.height {
        return Err(ManifestError::MinSizeExceedsDefault(raw.id));
    }
    let max_size = raw.max_size.as_ref().map(to_grid_size);
    if let Some(max_size) = max_size
        && (max_size.width < default_size.width || max_size.height < default_size.height)
    {
        return Err(ManifestError::MaxSizeBelowDefault(raw.id));
    }
    let data_source = match raw.data_source.as_str() {
        "mock" => DataSource::Mock,
        "live" => DataSource::Live,
        other => return Err(ManifestError::InvalidDataSource(raw.id, other.to_owned())),
    };
    if raw.entry.trim().is_empty() {
        return Err(ManifestError::EmptyEntry(raw.id));
    }
    validate_relative_path(&raw.entry)
        .map_err(|()| ManifestError::UnsafeEntryPath(raw.id.clone(), raw.entry.clone()))?;
    if !raw.config_schema.is_null() && !raw.config_schema.is_object() {
        return Err(ManifestError::InvalidConfigSchema(raw.id));
    }
    let config_schema = if raw.config_schema.is_null() {
        serde_json::Value::Object(serde_json::Map::new())
    } else {
        raw.config_schema
    };

    Ok(ValidatedWidgetContribution {
        widget_type_id: format!("{provider_id}/{}", raw.id),
        widget_id: raw.id,
        title: raw.title,
        description: raw.description,
        category: raw.category,
        default_size,
        min_size,
        max_size,
        config_schema_version: raw.config_schema_version,
        config_schema,
        required_permissions: raw.required_permissions,
        required_capabilities: raw.required_capabilities,
        data_source,
        entry: raw.entry,
    })
}

fn to_grid_size(raw: &RawGridSize) -> GridSize {
    GridSize {
        width: raw.width,
        height: raw.height,
    }
}

/// `Err(())` if `path` is absolute or contains a `..` component — the same
/// check [`crate::widget_package::store`] applies again at asset-resolution
/// time, on the theory that a rule this load-bearing is worth stating
/// twice.
fn validate_relative_path(path: &str) -> Result<(), ()> {
    let path = Path::new(path);
    if path.is_absolute() {
        return Err(());
    }
    if path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::Prefix(_) | Component::RootDir
        )
    }) {
        return Err(());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ManifestError, validate};

    fn manifest_json(contributes: &str) -> String {
        format!(
            r#"{{
                "id": "acme-widgets",
                "name": "Acme Widgets",
                "version": "1.0.0",
                "description": "example widgets",
                "contributes": [{contributes}]
            }}"#
        )
    }

    fn valid_widget_contribution() -> String {
        r#"{
            "point": "dashboard.widget",
            "widget": {
                "apiVersion": "senken.widget/v1",
                "id": "clock",
                "title": "Clock",
                "description": "shows the time",
                "category": "utility",
                "defaultSize": { "width": 3, "height": 2 },
                "minSize": { "width": 2, "height": 2 },
                "dataSource": "live",
                "entry": "index.html"
            }
        }"#
        .to_owned()
    }

    #[test]
    fn a_well_formed_dashboard_widget_manifest_validates() {
        let manifest = validate(manifest_json(&valid_widget_contribution()).as_bytes()).unwrap();
        assert_eq!(manifest.provider_id, "acme-widgets");
        assert_eq!(manifest.widgets.len(), 1);
        let widget = &manifest.widgets[0];
        assert_eq!(widget.widget_type_id, "acme-widgets/clock");
        assert_eq!(widget.default_size.width, 3);
        assert_eq!(widget.data_source, super::DataSource::Live);
    }

    #[test]
    fn malformed_json_is_rejected_by_name() {
        let err = validate(b"not json").unwrap_err();
        assert!(matches!(err, ManifestError::Malformed(_)));
    }

    #[test]
    fn an_invalid_provider_id_is_rejected() {
        let bad =
            manifest_json(&valid_widget_contribution()).replace("acme-widgets", "Acme Widgets!");
        let err = validate(bad.as_bytes()).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidProviderId(_)));
    }

    #[test]
    fn a_manifest_with_no_contributions_is_rejected() {
        let manifest = r#"{
            "id": "acme-widgets",
            "name": "Acme Widgets",
            "version": "1.0.0",
            "contributes": []
        }"#;
        let err = validate(manifest.as_bytes()).unwrap_err();
        assert!(matches!(err, ManifestError::NoContributions));
    }

    #[test]
    fn an_unknown_extension_point_is_rejected_by_name_not_silently_ignored() {
        let manifest = manifest_json(r#"{ "point": "footer.banner" }"#);
        let err = validate(manifest.as_bytes()).unwrap_err();
        assert_eq!(
            err,
            ManifestError::UnknownExtensionPoint("footer.banner".to_owned())
        );
    }

    #[test]
    fn a_reserved_but_unrouted_extension_point_is_rejected_and_says_so() {
        for point in [
            "chart.toolbar.item",
            "statusbar.item",
            "topbar.item",
            "settings.section",
        ] {
            let manifest = manifest_json(&format!(r#"{{ "point": "{point}" }}"#));
            let err = validate(manifest.as_bytes()).unwrap_err();
            assert_eq!(
                err,
                ManifestError::ExtensionPointNotYetAvailable(point.to_owned()),
                "point {point} must be reported as reserved, not as merely unknown"
            );
        }
    }

    #[test]
    fn a_dashboard_widget_contribution_with_no_widget_object_is_rejected() {
        let manifest = manifest_json(r#"{ "point": "dashboard.widget" }"#);
        let err = validate(manifest.as_bytes()).unwrap_err();
        assert!(matches!(err, ManifestError::MissingWidgetDefinition));
    }

    #[test]
    fn a_provider_id_in_the_widget_object_cannot_override_the_derived_one() {
        // The wire schema has no `providerId` field on the widget object at
        // all — extra/unknown fields are ignored by `serde`'s default
        // behavior, so even a plugin author who copies the design doc's
        // TypeScript interface verbatim (which does carry `providerId`)
        // cannot make their widget claim a different provider.
        let contribution = valid_widget_contribution().replace(
            "\"id\": \"clock\",",
            "\"id\": \"clock\", \"providerId\": \"someone-else\",",
        );
        let manifest = validate(manifest_json(&contribution).as_bytes()).unwrap();
        assert_eq!(manifest.widgets[0].widget_type_id, "acme-widgets/clock");
    }

    #[test]
    fn duplicate_widget_ids_in_one_package_are_rejected() {
        let manifest = manifest_json(&format!(
            "{}, {}",
            valid_widget_contribution(),
            valid_widget_contribution()
        ));
        let err = validate(manifest.as_bytes()).unwrap_err();
        assert!(matches!(err, ManifestError::DuplicateWidgetId(id) if id == "clock"));
    }

    #[test]
    fn an_unsupported_api_version_is_rejected() {
        let contribution =
            valid_widget_contribution().replace("senken.widget/v1", "senken.widget/v2");
        let err = validate(manifest_json(&contribution).as_bytes()).unwrap_err();
        assert!(
            matches!(err, ManifestError::UnsupportedApiVersion(_, v) if v == "senken.widget/v2")
        );
    }

    #[test]
    fn a_min_size_larger_than_default_size_is_rejected() {
        let contribution = valid_widget_contribution().replace(
            r#""minSize": { "width": 2, "height": 2 }"#,
            r#""minSize": { "width": 9, "height": 9 }"#,
        );
        let err = validate(manifest_json(&contribution).as_bytes()).unwrap_err();
        assert!(matches!(err, ManifestError::MinSizeExceedsDefault(_)));
    }

    #[test]
    fn a_zero_sized_default_is_rejected() {
        let contribution = valid_widget_contribution().replace(
            r#""defaultSize": { "width": 3, "height": 2 }"#,
            r#""defaultSize": { "width": 0, "height": 2 }"#,
        );
        let err = validate(manifest_json(&contribution).as_bytes()).unwrap_err();
        assert!(matches!(err, ManifestError::ZeroSize(_, "defaultSize")));
    }

    #[test]
    fn an_invalid_data_source_is_rejected() {
        let contribution = valid_widget_contribution()
            .replace("\"dataSource\": \"live\"", "\"dataSource\": \"synthetic\"");
        let err = validate(manifest_json(&contribution).as_bytes()).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidDataSource(_, ds) if ds == "synthetic"));
    }

    #[test]
    fn a_path_traversal_entry_is_rejected() {
        for escaping in ["../secrets.html", "/etc/passwd", "web/../../escape.html"] {
            let contribution = valid_widget_contribution().replace(
                "\"entry\": \"index.html\"",
                &format!("\"entry\": \"{escaping}\""),
            );
            let err = validate(manifest_json(&contribution).as_bytes()).unwrap_err();
            assert!(
                matches!(err, ManifestError::UnsafeEntryPath(_, _)),
                "{escaping} must be rejected as an unsafe entry path, got {err:?}"
            );
        }
    }

    #[test]
    fn a_non_object_config_schema_is_rejected() {
        let contribution = valid_widget_contribution().replacen(
            "\"dataSource\"",
            "\"configSchema\": \"not an object\", \"dataSource\"",
            1,
        );
        let err = validate(manifest_json(&contribution).as_bytes()).unwrap_err();
        assert!(matches!(err, ManifestError::InvalidConfigSchema(_)));
    }
}
