//! The dynamic widget UI package contract: a plugin author ships a
//! self-contained bundle (`manifest.json` plus a `web/` directory of static
//! assets) built entirely outside this repository, and the host serves it
//! into a sandboxed iframe without ever compiling or executing it directly.
//!
//! This is deliberately a different shape from [`crate::Plugin`]: that
//! trait is for a statically-linked Rust crate compiled into this binary
//! (`impl Plugin` + `cargo build`). A widget UI package is the opposite —
//! **no Rust, no compilation step on this host at all** — so it lives in
//! its own module here rather than as a third [`crate::Plugin`] activation
//! path. What the two share is only the idea of "the definition of what a
//! plugin author must provide"; nothing else about how they load or run is
//! shared, and nothing here calls into [`crate::ActivationContext`].
//!
//! # Two pieces
//!
//! - [`crate::widget_package::manifest::validate`] turns an untrusted
//!   `manifest.json`'s bytes into a
//!   [`crate::widget_package::ValidatedManifest`] — pure, no I/O, no
//!   filesystem.
//! - [`crate::widget_package::WidgetPackageStore`] owns the on-disk
//!   directory of installed packages: install (from an uploaded zip
//!   archive), discovery/refresh, enable/disable, and the effective
//!   catalog of widgets every currently enabled-and-valid package
//!   contributes.
//!
//! # Why disable never touches a placed widget's config
//!
//! This store is the thing that goes silent when a package is disabled —
//! [`crate::widget_package::WidgetPackageStore::effective_widget_catalog`]
//! simply stops naming that package's widgets. It never deletes the
//! package's files and
//! never reaches into wherever a widget instance's config is stored (that
//! is `senken_dashboard`'s job, over in a different crate this one does not
//! depend on). A caller cross-references a stored `widget_type_id` against
//! this store's catalog the same way it would against
//! `senken_dashboard::WidgetRegistry` — when the lookup misses, the caller
//! draws a placeholder and leaves the stored config exactly where it was.

/// The manifest schema and its validation. See this module's own docs.
pub mod manifest;
/// The on-disk package store: install, discovery, enable/disable, and the
/// effective widget catalog. See this module's own docs.
pub mod store;

pub use manifest::{
    DASHBOARD_WIDGET_POINT, DataSource, GridSize, ManifestError, ValidatedManifest,
    ValidatedWidgetContribution,
};
pub use store::{InstalledPackage, PackageStatus, WidgetPackageError, WidgetPackageStore};
