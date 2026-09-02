//! [`WidgetPackageStore`]: the on-disk directory of installed widget UI
//! packages, and the effective catalog a dashboard's grid checks a placed
//! widget's `widget_type_id` against.
//!
//! # Layout
//!
//! ```text
//! <data_dir>/widget-plugins/
//!   state.json                 -- { "<package id>": <enabled bool>, .. }
//!   packages/
//!     <package id>/
//!       manifest.json
//!       web/
//!         index.html
//!         ...
//! ```
//!
//! Nothing here is cached in memory: every method re-reads the filesystem,
//! so there is no cache to invalidate and no filesystem watcher to race a
//! half-written upload — the same "refresh is explicit, not a watcher"
//! decision this platform's design record makes for every other kind of
//! plugin, for the same reason (a watcher firing mid-copy reads a half
//! written file).
//!
//! # Install is atomic
//!
//! [`WidgetPackageStore::install`] extracts an uploaded zip archive into a
//! staging directory first, validates the manifest and every widget's own
//! entry file *there*, and only then moves the whole directory into place
//! with one `rename` — a single atomic filesystem operation on the same
//! volume. A failure at any point during extraction or validation leaves
//! the previously installed package (if any) completely untouched; the
//! staging directory is removed rather than ever becoming visible to
//! [`WidgetPackageStore::list`].

use std::collections::HashMap;
use std::io::{Cursor, Write as _};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use senken_storage::{Snapshot, Storage, StorageError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;

use super::manifest::{self, ManifestError, ValidatedWidgetContribution};

/// The schema version [`PackageStateFile`] is written under.
const STATE_SCHEMA_VERSION: u32 = 1;

/// The package id reserved for the widget UI package this server installs
/// on every fresh start, so the dashboard's "add widget" picker and the
/// widget-plugin manager show a real, working plugin from the first run —
/// not an empty list nobody has uploaded anything to yet. It is exactly
/// `examples/widget-plugins/example-clock` (compiled in rather than
/// requiring an upload), the same package `examples/widget-plugins/README.md`
/// tells a plugin author to build one like. `example-quotes`, the other
/// example there, is deliberately left uninstalled: its zip stays sitting
/// next to it precisely so there is still something to try the upload flow
/// with on a fresh install. [`WidgetPackageStore::uninstall`] refuses this
/// id (an admin disables it instead, the same remedy a built-in indicator
/// plugin gets in `senken_runtime::plugin_host::PluginOrigin`); everything
/// else — enable, disable, refresh — goes through the exact same path an
/// uploaded package does.
pub const BUILTIN_PACKAGE_ID: &str = "example-clock";

/// This build's own compiled-in `manifest.json` for [`BUILTIN_PACKAGE_ID`]
/// — the checked-in `examples/widget-plugins/example-clock/manifest.json`,
/// not a copy, so the built-in can never drift from the example it is.
const BUILTIN_MANIFEST: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/widget-plugins/example-clock/manifest.json"
));
/// This build's own compiled-in `web/index.html` for [`BUILTIN_PACKAGE_ID`]
/// — see [`BUILTIN_MANIFEST`].
const BUILTIN_INDEX_HTML: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../examples/widget-plugins/example-clock/web/index.html"
));

/// Packs [`BUILTIN_MANIFEST`] and [`BUILTIN_INDEX_HTML`] into the same
/// zip-archive shape [`WidgetPackageStore::install`] expects from an
/// upload, so the built-in package goes through the exact same validated,
/// atomic install path as one a plugin author zipped up by hand — no
/// second way for a package to land on disk.
fn builtin_package_archive() -> Vec<u8> {
    let mut buffer = Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        // Both writes are to an in-memory `Cursor` over bytes this binary
        // itself compiled in — a failure here would mean a broken build,
        // not a caller mistake, so `expect` names exactly that.
        writer
            .start_file("manifest.json", options)
            .expect("in-memory zip write");
        writer
            .write_all(BUILTIN_MANIFEST.as_bytes())
            .expect("in-memory zip write");
        writer
            .start_file("web/index.html", options)
            .expect("in-memory zip write");
        writer
            .write_all(BUILTIN_INDEX_HTML.as_bytes())
            .expect("in-memory zip write");
        writer.finish().expect("in-memory zip write");
    }
    buffer.into_inner()
}

/// An uploaded archive with more entries than this is rejected outright,
/// before a single byte is extracted — a cheap bound against a
/// pathologically large listing.
const MAX_ARCHIVE_ENTRIES: usize = 512;

/// An uploaded archive whose extracted contents would exceed this many
/// bytes is rejected mid-extraction — a bound against a zip bomb, since a
/// small compressed archive can otherwise expand to an enormous one.
const MAX_TOTAL_UNCOMPRESSED_BYTES: u64 = 32 * 1024 * 1024;

/// Everything that can go wrong operating on the widget package store.
#[derive(Debug, thiserror::Error)]
pub enum WidgetPackageError {
    /// The small `state.json` file could not be read or written.
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// A package's `manifest.json` failed validation.
    #[error(transparent)]
    Manifest(#[from] ManifestError),
    /// The uploaded bytes are not a valid zip archive, or violate one of
    /// this store's own limits (entry count, total size).
    #[error("package archive is invalid: {0}")]
    InvalidArchive(String),
    /// An archive entry's path is absolute or attempts to escape the
    /// package directory (`..`) — rejected before any byte is written to
    /// disk.
    #[error("package archive contains an unsafe path: {0:?}")]
    UnsafeArchiveEntry(String),
    /// The archive has no `manifest.json` at its root.
    #[error("package archive has no manifest.json at its root")]
    MissingManifest,
    /// A widget's declared `entry` path is not actually present under the
    /// archive's `web/` directory.
    #[error("widget {0:?} names entry {1:?}, which the archive's web/ directory does not contain")]
    EntryNotFound(String, String),
    /// No package with this id is installed.
    #[error("no widget plugin package with id {0:?} is installed")]
    NotFound(String),
    /// A caller tried to [`WidgetPackageStore::uninstall`] the package
    /// reserved for [`BUILTIN_PACKAGE_ID`]. Disabling it is still allowed —
    /// only removing its files outright is not.
    #[error("the built-in {0:?} package cannot be uninstalled; disable it instead")]
    CannotUninstallBuiltIn(String),
    /// A requested asset path is absolute or attempts to escape the
    /// package's own `web/` directory.
    #[error("asset path {0:?} is not a valid relative path")]
    UnsafeAssetPath(String),
    /// An underlying filesystem operation failed.
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
}

/// Where an installed package currently stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageStatus {
    /// Enabled, and its manifest validated cleanly — its widgets are in the
    /// effective catalog and any placed instance of one of them renders
    /// for real.
    Active,
    /// An admin has disabled this package. Its widgets are not in the
    /// effective catalog; a placed instance of one of them renders as a
    /// placeholder until this package is enabled again. Its files and its
    /// widgets' declared metadata are untouched either way.
    Disabled,
    /// Enabled, but its manifest failed to validate (or its files are
    /// unreadable) — contributes nothing, same as [`Self::Disabled`] from
    /// an effective-catalog point of view, but distinguished so the reason
    /// can be shown to whoever installed it.
    Failed(String),
}

/// One installed widget UI package, as read back by
/// [`WidgetPackageStore::list`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPackage {
    /// The package's own id (its install-directory name, and every
    /// contributed widget's provider id).
    pub id: String,
    /// Display name, from the manifest. Empty if the manifest could not be
    /// parsed at all.
    pub name: String,
    /// The package's own version string, from the manifest.
    pub version: String,
    /// A one-line description, from the manifest.
    pub description: String,
    /// The admin-controlled enable/disable flag, independent of whether the
    /// manifest currently validates.
    pub enabled: bool,
    /// This package's current status — see [`PackageStatus`].
    pub status: PackageStatus,
    /// A SHA-256 hex digest of this package's own `manifest.json` bytes,
    /// for an admin to confirm what is actually installed matches what was
    /// meant to be uploaded.
    pub digest: String,
    /// Every `dashboard.widget` this package declares — populated only
    /// when [`Self::status`] is [`PackageStatus::Active`]; empty otherwise,
    /// since a disabled or failed package contributes nothing to the
    /// effective catalog.
    pub widgets: Vec<ValidatedWidgetContribution>,
    /// `true` for [`BUILTIN_PACKAGE_ID`] — ships with this server rather
    /// than having been uploaded or dropped into the data directory by
    /// hand. Derived from the id alone (no separate on-disk marker to ever
    /// drift from it); an admin may still disable a built-in, just never
    /// uninstall one — see [`WidgetPackageStore::uninstall`].
    pub is_builtin: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct PackageStateFile {
    /// Package id -> the admin's enable/disable flag.
    enabled: HashMap<String, bool>,
}

/// The on-disk directory of installed widget UI packages. See this
/// module's own docs for the layout and the atomicity guarantee
/// [`install`](Self::install) makes.
#[derive(Debug, Clone)]
pub struct WidgetPackageStore {
    root: PathBuf,
    state: Storage,
}

impl WidgetPackageStore {
    /// Opens (creating if necessary) the widget-plugin package store rooted
    /// at `<data_dir>/widget-plugins`.
    ///
    /// # Errors
    /// [`WidgetPackageError::Io`] if the directory cannot be created.
    pub fn open(data_dir: impl AsRef<Path>) -> Result<Self, WidgetPackageError> {
        let root = data_dir.as_ref().join("widget-plugins");
        std::fs::create_dir_all(root.join("packages"))?;
        let state = Storage::new(&root);
        state.init()?;
        Ok(Self { root, state })
    }

    fn packages_dir(&self) -> PathBuf {
        self.root.join("packages")
    }

    fn read_state(&self) -> Result<PackageStateFile, WidgetPackageError> {
        Ok(self
            .state
            .read_snapshot::<PackageStateFile>("state.json", STATE_SCHEMA_VERSION)?
            .map(|s| s.data)
            .unwrap_or_default())
    }

    fn write_state(&self, state: &PackageStateFile) -> Result<(), WidgetPackageError> {
        self.state
            .write_snapshot("state.json", &Snapshot::new(STATE_SCHEMA_VERSION, state))?;
        Ok(())
    }

    /// Every installed package, in a stable order (`id` ascending) so a
    /// listing does not reorder itself between two calls with nothing
    /// changed.
    ///
    /// Re-reads the filesystem every call — see this module's docs on why
    /// there is no cache.
    ///
    /// # Errors
    /// [`WidgetPackageError::Io`]/[`WidgetPackageError::Storage`] if the
    /// package directory or the state file cannot be read. A single
    /// package's own manifest failing to parse is **not** an error here —
    /// it is reported as that one package's [`PackageStatus::Failed`], so
    /// one broken package can never hide every other one from the listing.
    pub fn list(&self) -> Result<Vec<InstalledPackage>, WidgetPackageError> {
        let state = self.read_state()?;
        let mut out = Vec::new();
        let dir = self.packages_dir();
        for entry in std::fs::read_dir(&dir)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            // Leftover staging directories from an aborted install are
            // dot-prefixed and never a real package id (see `install`).
            if dir_name.starts_with('.') {
                continue;
            }
            out.push(Self::load_one(&entry.path(), dir_name, &state));
        }
        out.sort_by(|a, b| a.id.cmp(&b.id));
        Ok(out)
    }

    /// Explicit rescan — an alias for [`list`](Self::list) kept as its own
    /// named entry point because the design this store follows insists a
    /// caller ask for a rescan on purpose (an admin action, a poll) rather
    /// than this store watching the filesystem itself: a watcher can fire
    /// while an upload is still being copied into place and read a
    /// half-written file. Since nothing here is cached, refreshing and
    /// listing are the same read.
    ///
    /// # Errors
    /// See [`list`](Self::list).
    pub fn refresh(&self) -> Result<Vec<InstalledPackage>, WidgetPackageError> {
        self.list()
    }

    /// Loads one package directory's status — an associated function
    /// rather than a method since it needs nothing from `self` beyond what
    /// [`list`](Self::list) already read.
    fn load_one(path: &Path, dir_name: String, state: &PackageStateFile) -> InstalledPackage {
        let enabled = state.enabled.get(&dir_name).copied().unwrap_or(true);
        let is_builtin = dir_name == BUILTIN_PACKAGE_ID;
        let manifest_bytes = match std::fs::read(path.join("manifest.json")) {
            Ok(bytes) => bytes,
            Err(source) => {
                return InstalledPackage {
                    id: dir_name.clone(),
                    name: dir_name,
                    version: String::new(),
                    description: String::new(),
                    enabled,
                    status: PackageStatus::Failed(format!("manifest.json unreadable: {source}")),
                    digest: String::new(),
                    widgets: Vec::new(),
                    is_builtin,
                };
            }
        };
        let digest = hex_sha256(&manifest_bytes);
        match manifest::validate(&manifest_bytes) {
            Ok(parsed) if parsed.provider_id == dir_name => {
                let status = if enabled {
                    PackageStatus::Active
                } else {
                    PackageStatus::Disabled
                };
                let widgets = if enabled { parsed.widgets } else { Vec::new() };
                InstalledPackage {
                    id: dir_name,
                    name: parsed.name,
                    version: parsed.version,
                    description: parsed.description,
                    enabled,
                    status,
                    digest,
                    widgets,
                    is_builtin,
                }
            }
            Ok(parsed) => InstalledPackage {
                id: dir_name.clone(),
                name: parsed.name,
                version: parsed.version,
                description: parsed.description,
                enabled,
                status: PackageStatus::Failed(format!(
                    "manifest id {:?} does not match its install directory {dir_name:?}",
                    parsed.provider_id
                )),
                digest,
                widgets: Vec::new(),
                is_builtin,
            },
            Err(source) => InstalledPackage {
                id: dir_name.clone(),
                name: dir_name,
                version: String::new(),
                description: String::new(),
                enabled,
                status: PackageStatus::Failed(source.to_string()),
                digest,
                widgets: Vec::new(),
                is_builtin,
            },
        }
    }

    /// Every widget every currently [`PackageStatus::Active`] package
    /// contributes — the list a dashboard's "add widget" picker and its
    /// grid's placeholder check draw from, merged with whatever built-in
    /// catalog the caller already has.
    ///
    /// # Errors
    /// See [`list`](Self::list).
    pub fn effective_widget_catalog(
        &self,
    ) -> Result<Vec<ValidatedWidgetContribution>, WidgetPackageError> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|p| p.status == PackageStatus::Active)
            .flat_map(|p| p.widgets)
            .collect())
    }

    /// Installs a package from the raw bytes of a zip archive (its
    /// `manifest.json` at the archive root, its static assets under
    /// `web/`).
    ///
    /// Extracts to a staging directory, validates the manifest and every
    /// widget's `entry` path there, and only then moves the staging
    /// directory into place with one `rename` — see this module's docs for
    /// why that is what makes install atomic. Re-installing the same
    /// package id **replaces** its files (an upgrade) but leaves an
    /// existing enable/disable flag exactly as an admin last set it, rather
    /// than silently re-enabling a package they had disabled.
    ///
    /// # Errors
    /// [`WidgetPackageError::InvalidArchive`]/
    /// [`WidgetPackageError::UnsafeArchiveEntry`]/
    /// [`WidgetPackageError::MissingManifest`]/
    /// [`WidgetPackageError::EntryNotFound`]/[`WidgetPackageError::Manifest`]
    /// if the archive or its manifest is invalid — in every case the
    /// staging directory is removed and nothing already installed is
    /// touched; otherwise as [`WidgetPackageError::Io`]/
    /// [`WidgetPackageError::Storage`].
    pub fn install(&self, zip_bytes: &[u8]) -> Result<String, WidgetPackageError> {
        let staging = self.packages_dir().join(format!(
            ".staging-{}-{}",
            std::process::id(),
            next_sequence()
        ));
        std::fs::create_dir_all(&staging)?;

        let outcome = extract_and_validate(zip_bytes, &staging);
        let provider_id = match outcome {
            Ok(id) => id,
            Err(source) => {
                let _ = std::fs::remove_dir_all(&staging);
                return Err(source);
            }
        };

        let final_dir = self.packages_dir().join(&provider_id);
        if final_dir.is_dir() {
            std::fs::remove_dir_all(&final_dir)?;
        }
        std::fs::rename(&staging, &final_dir)?;

        // A brand new id starts enabled; an id being re-installed keeps
        // whatever an admin last set.
        let mut state = self.read_state()?;
        state.enabled.entry(provider_id.clone()).or_insert(true);
        self.write_state(&state)?;

        Ok(provider_id)
    }

    /// Sets the admin enable/disable flag for `id`. Never touches this
    /// package's files, and never touches anything a dashboard has stored
    /// about a placed instance of one of its widgets — see this module's
    /// docs.
    ///
    /// # Errors
    /// [`WidgetPackageError::NotFound`] if no package with this id is
    /// installed; otherwise as [`WidgetPackageError::Storage`].
    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<(), WidgetPackageError> {
        if !self.packages_dir().join(id).is_dir() {
            return Err(WidgetPackageError::NotFound(id.to_owned()));
        }
        let mut state = self.read_state()?;
        state.enabled.insert(id.to_owned(), enabled);
        self.write_state(&state)
    }

    /// Removes a package's files entirely, and forgets its enable/disable
    /// flag. Unlike disabling, this cannot be undone by re-enabling; a
    /// removed package must be re-installed from its archive again.
    ///
    /// # Errors
    /// [`WidgetPackageError::NotFound`] if no package with this id is
    /// installed; otherwise as [`WidgetPackageError::Io`]/
    /// [`WidgetPackageError::Storage`].
    pub fn uninstall(&self, id: &str) -> Result<(), WidgetPackageError> {
        if id == BUILTIN_PACKAGE_ID {
            return Err(WidgetPackageError::CannotUninstallBuiltIn(id.to_owned()));
        }
        let dir = self.packages_dir().join(id);
        if !dir.is_dir() {
            return Err(WidgetPackageError::NotFound(id.to_owned()));
        }
        std::fs::remove_dir_all(&dir)?;
        let mut state = self.read_state()?;
        state.enabled.remove(id);
        self.write_state(&state)
    }

    /// Installs the built-in [`BUILTIN_PACKAGE_ID`] package if it is not
    /// already present on disk — called once by the runtime at startup,
    /// never by [`open`](Self::open) itself, so a caller opening a store
    /// purely to test or inspect it (as every test in this module does)
    /// never gets a package it did not ask for.
    ///
    /// Goes through [`install`](Self::install) exactly like an upload
    /// would, so the built-in gets the same validated, atomic path rather
    /// than a second one that could drift from it. Already being present
    /// (from an earlier run) is a no-op, not a reinstall — an admin's own
    /// disable, or a future version's changed defaults saved into config,
    /// is never silently clobbered by this running again on every start.
    ///
    /// # Errors
    /// As [`install`](Self::install) — a failure here means this binary
    /// shipped a broken built-in, not a caller mistake.
    pub fn ensure_builtin_installed(&self) -> Result<(), WidgetPackageError> {
        if self.packages_dir().join(BUILTIN_PACKAGE_ID).is_dir() {
            return Ok(());
        }
        self.install(&builtin_package_archive())?;
        Ok(())
    }

    /// Resolves `rel_path` (relative to package `id`'s own `web/`
    /// directory) to a real file on disk, for the asset server to read and
    /// stream back. Returns `Ok(None)` for a package that does not exist,
    /// is not currently [`PackageStatus::Active`], or for a path that does
    /// not name a real file — a disabled or failed package's assets are
    /// never served, exactly like its widgets are never in the effective
    /// catalog.
    ///
    /// # Errors
    /// [`WidgetPackageError::UnsafeAssetPath`] if `rel_path` is absolute or
    /// attempts to escape the package directory; otherwise as
    /// [`list`](Self::list).
    pub fn resolve_asset(
        &self,
        id: &str,
        rel_path: &str,
    ) -> Result<Option<PathBuf>, WidgetPackageError> {
        let rel = Path::new(rel_path);
        if !is_safe_relative_path(rel) {
            return Err(WidgetPackageError::UnsafeAssetPath(rel_path.to_owned()));
        }
        let is_active = self
            .list()?
            .into_iter()
            .any(|p| p.id == id && p.status == PackageStatus::Active);
        if !is_active {
            return Ok(None);
        }
        let full = self.packages_dir().join(id).join("web").join(rel);
        Ok(full.is_file().then_some(full))
    }
}

/// Extracts `zip_bytes` into `staging`, then validates the manifest and
/// every widget's `entry` path — the whole fallible body of
/// [`WidgetPackageStore::install`], split out so that function's own
/// staging-directory cleanup has one call to wrap.
fn extract_and_validate(zip_bytes: &[u8], staging: &Path) -> Result<String, WidgetPackageError> {
    let mut archive = zip::ZipArchive::new(Cursor::new(zip_bytes))
        .map_err(|source| WidgetPackageError::InvalidArchive(source.to_string()))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(WidgetPackageError::InvalidArchive(format!(
            "archive has {} entries, more than the {MAX_ARCHIVE_ENTRIES} limit",
            archive.len()
        )));
    }

    let mut total_bytes: u64 = 0;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|source| WidgetPackageError::InvalidArchive(source.to_string()))?;
        if file.is_dir() {
            continue;
        }
        let name = file.name().to_owned();
        let rel = Path::new(&name);
        if !is_safe_relative_path(rel) {
            return Err(WidgetPackageError::UnsafeArchiveEntry(name));
        }
        total_bytes = total_bytes.saturating_add(file.size());
        if total_bytes > MAX_TOTAL_UNCOMPRESSED_BYTES {
            return Err(WidgetPackageError::InvalidArchive(format!(
                "archive expands past the {MAX_TOTAL_UNCOMPRESSED_BYTES}-byte limit"
            )));
        }
        let dest = staging.join(rel);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&dest)?;
        std::io::copy(&mut file, &mut out)?;
    }

    let manifest_bytes = std::fs::read(staging.join("manifest.json"))
        .map_err(|_| WidgetPackageError::MissingManifest)?;
    let manifest = manifest::validate(&manifest_bytes)?;
    for widget in &manifest.widgets {
        let entry_path = staging.join("web").join(&widget.entry);
        if !entry_path.is_file() {
            return Err(WidgetPackageError::EntryNotFound(
                widget.widget_id.clone(),
                widget.entry.clone(),
            ));
        }
    }
    Ok(manifest.provider_id)
}

/// `false` if `path` is absolute or contains a `..` component. Shared by
/// archive-entry validation at install time and asset-path validation at
/// serve time — two different callers, one rule.
fn is_safe_relative_path(path: &Path) -> bool {
    if path.as_os_str().is_empty() {
        return false;
    }
    !path.is_absolute()
        && !path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::Prefix(_) | Component::RootDir
            )
        })
}

fn hex_sha256(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write;
        let _ = write!(acc, "{b:02x}");
        acc
    })
}

/// A counter unique per process, so two staging directories created in the
/// same millisecond never collide.
fn next_sequence() -> u64 {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use std::io::Write as _;

    use tempfile::TempDir;
    use zip::write::SimpleFileOptions;

    use super::{PackageStatus, WidgetPackageError, WidgetPackageStore};

    /// Builds a minimal, valid widget package archive: a `manifest.json`
    /// declaring one `dashboard.widget` contribution named `clock`, plus a
    /// `web/index.html` its `entry` names.
    fn valid_package_zip(provider_id: &str) -> Vec<u8> {
        build_zip(&[
            ("manifest.json", manifest_json(provider_id).as_bytes()),
            ("web/index.html", b"<!doctype html><title>clock</title>"),
        ])
    }

    fn manifest_json(provider_id: &str) -> String {
        format!(
            r#"{{
                "id": "{provider_id}",
                "name": "Example Widgets",
                "version": "1.0.0",
                "contributes": [{{
                    "point": "dashboard.widget",
                    "widget": {{
                        "apiVersion": "senken.widget/v1",
                        "id": "clock",
                        "title": "Clock",
                        "defaultSize": {{ "width": 3, "height": 2 }},
                        "minSize": {{ "width": 2, "height": 2 }},
                        "dataSource": "live",
                        "entry": "index.html"
                    }}
                }}]
            }}"#
        )
    }

    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut buf = std::io::Cursor::new(Vec::new());
        {
            let mut writer = zip::ZipWriter::new(&mut buf);
            let options =
                SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            for (name, bytes) in entries {
                writer.start_file(*name, options).unwrap();
                writer.write_all(bytes).unwrap();
            }
            writer.finish().unwrap();
        }
        buf.into_inner()
    }

    fn temp_store() -> (TempDir, WidgetPackageStore) {
        let dir = TempDir::new().unwrap();
        let store = WidgetPackageStore::open(dir.path()).unwrap();
        (dir, store)
    }

    #[test]
    fn installing_a_valid_package_makes_its_widget_active_and_in_the_effective_catalog() {
        let (_dir, store) = temp_store();
        let id = store.install(&valid_package_zip("acme-widgets")).unwrap();
        assert_eq!(id, "acme-widgets");

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].status, PackageStatus::Active);
        assert!(listed[0].enabled);
        assert_eq!(listed[0].widgets.len(), 1);
        assert_eq!(listed[0].widgets[0].widget_type_id, "acme-widgets/clock");

        let catalog = store.effective_widget_catalog().unwrap();
        assert_eq!(catalog.len(), 1);
        assert_eq!(catalog[0].widget_type_id, "acme-widgets/clock");
    }

    #[test]
    fn disabling_removes_the_widget_from_the_catalog_and_enabling_restores_it_unchanged() {
        let (_dir, store) = temp_store();
        store.install(&valid_package_zip("acme-widgets")).unwrap();
        let original = store.list().unwrap()[0].widgets.clone();
        assert!(!original.is_empty());

        // Mutate first: prove the property actually catches the thing.
        // Before disabling, the widget is indeed reachable through the
        // catalog a dashboard picker would read.
        assert_eq!(store.effective_widget_catalog().unwrap().len(), 1);

        store.set_enabled("acme-widgets", false).unwrap();
        let disabled = store.list().unwrap();
        assert_eq!(disabled[0].status, PackageStatus::Disabled);
        assert!(!disabled[0].enabled);
        assert!(
            disabled[0].widgets.is_empty(),
            "a disabled package must contribute nothing to what a caller reads back"
        );
        assert!(
            store.effective_widget_catalog().unwrap().is_empty(),
            "the effective catalog must be empty while disabled"
        );
        // Disabling must not touch the package's own files.
        assert_eq!(disabled[0].name, "Example Widgets");
        assert_eq!(disabled[0].version, "1.0.0");

        store.set_enabled("acme-widgets", true).unwrap();
        let restored = store.list().unwrap();
        assert_eq!(restored[0].status, PackageStatus::Active);
        assert_eq!(
            restored[0].widgets, original,
            "re-enabling must restore the exact same widget definition, unharmed"
        );
    }

    #[test]
    fn a_package_with_an_id_mismatched_to_its_directory_is_reported_failed_not_silently_dropped() {
        let (dir, store) = temp_store();
        store.install(&valid_package_zip("acme-widgets")).unwrap();
        // Simulate corruption: rename the install directory so it no
        // longer matches the manifest's own `id`.
        let packages = dir.path().join("widget-plugins").join("packages");
        std::fs::rename(
            packages.join("acme-widgets"),
            packages.join("renamed-widgets"),
        )
        .unwrap();

        let listed = store.list().unwrap();
        assert_eq!(listed.len(), 1);
        assert!(
            matches!(&listed[0].status, PackageStatus::Failed(reason) if reason.contains("does not match")),
            "got {:?}",
            listed[0].status
        );
        assert!(listed[0].widgets.is_empty());
        assert!(store.effective_widget_catalog().unwrap().is_empty());
    }

    #[test]
    fn a_path_traversal_entry_in_the_archive_is_rejected_and_leaves_no_trace() {
        let (dir, store) = temp_store();
        let evil = build_zip(&[
            ("manifest.json", manifest_json("evil-widgets").as_bytes()),
            ("../../escape.txt", b"escaped"),
        ]);
        let err = store.install(&evil).unwrap_err();
        assert!(matches!(err, WidgetPackageError::UnsafeArchiveEntry(_)));

        // Nothing must be left behind: no installed package, no staging
        // directory, and nothing outside the store's own root either.
        assert!(store.list().unwrap().is_empty());
        let packages_dir = dir.path().join("widget-plugins").join("packages");
        let leftover: Vec<_> = std::fs::read_dir(&packages_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert!(
            leftover.is_empty(),
            "install must clean up its staging directory on failure, found {leftover:?}"
        );
        assert!(!dir.path().join("escape.txt").exists());
    }

    #[test]
    fn an_archive_with_no_manifest_is_rejected() {
        let (_dir, store) = temp_store();
        let bad = build_zip(&[("web/index.html", b"<html></html>")]);
        let err = store.install(&bad).unwrap_err();
        assert!(matches!(err, WidgetPackageError::MissingManifest));
    }

    #[test]
    fn a_widget_naming_an_entry_the_archive_does_not_contain_is_rejected() {
        let (_dir, store) = temp_store();
        // Same manifest as `valid_package_zip`, but the `web/index.html`
        // its `entry` names is missing from the archive entirely.
        let bad = build_zip(&[("manifest.json", manifest_json("acme-widgets").as_bytes())]);
        let err = store.install(&bad).unwrap_err();
        assert!(
            matches!(err, WidgetPackageError::EntryNotFound(id, entry) if id == "clock" && entry == "index.html")
        );
    }

    #[test]
    fn reinstalling_the_same_id_replaces_files_but_keeps_an_existing_disabled_flag() {
        let (_dir, store) = temp_store();
        store.install(&valid_package_zip("acme-widgets")).unwrap();
        store.set_enabled("acme-widgets", false).unwrap();

        store.install(&valid_package_zip("acme-widgets")).unwrap();
        let listed = store.list().unwrap();
        assert_eq!(
            listed[0].status,
            PackageStatus::Disabled,
            "re-installing must not silently re-enable a package an admin disabled"
        );
    }

    #[test]
    fn resolve_asset_serves_only_for_an_active_package_and_rejects_traversal() {
        let (_dir, store) = temp_store();
        store.install(&valid_package_zip("acme-widgets")).unwrap();

        let resolved = store.resolve_asset("acme-widgets", "index.html").unwrap();
        assert!(resolved.is_some());
        assert!(
            std::fs::read_to_string(resolved.unwrap())
                .unwrap()
                .contains("clock")
        );

        assert_eq!(
            store.resolve_asset("acme-widgets", "missing.js").unwrap(),
            None
        );

        store.set_enabled("acme-widgets", false).unwrap();
        assert_eq!(
            store.resolve_asset("acme-widgets", "index.html").unwrap(),
            None,
            "a disabled package's assets must not be served"
        );

        let err = store
            .resolve_asset("acme-widgets", "../../../etc/passwd")
            .unwrap_err();
        assert!(matches!(err, WidgetPackageError::UnsafeAssetPath(_)));
    }

    #[test]
    fn uninstall_removes_files_and_forgets_the_enabled_flag() {
        let (dir, store) = temp_store();
        store.install(&valid_package_zip("acme-widgets")).unwrap();
        store.uninstall("acme-widgets").unwrap();

        assert!(store.list().unwrap().is_empty());
        assert!(
            !dir.path()
                .join("widget-plugins/packages/acme-widgets")
                .exists()
        );
        let err = store.set_enabled("acme-widgets", true).unwrap_err();
        assert!(matches!(err, WidgetPackageError::NotFound(_)));
    }

    #[test]
    fn ensure_builtin_installed_registers_a_widget_on_a_fresh_store() {
        let (_dir, store) = temp_store();
        assert!(
            store.list().unwrap().is_empty(),
            "a fresh store must start with nothing installed"
        );

        store.ensure_builtin_installed().unwrap();

        let packages = store.list().unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(packages[0].id, super::BUILTIN_PACKAGE_ID);
        assert!(packages[0].is_builtin);
        assert_eq!(packages[0].status, PackageStatus::Active);
        assert!(
            !packages[0].widgets.is_empty(),
            "the built-in package must contribute at least one widget"
        );

        let catalog = store.effective_widget_catalog().unwrap();
        assert!(
            catalog
                .iter()
                .any(|w| w.widget_type_id.starts_with(super::BUILTIN_PACKAGE_ID)),
            "the built-in's widget must reach the effective catalog same as any other package's"
        );
    }

    #[test]
    fn ensure_builtin_installed_is_a_no_op_once_already_present() {
        let (_dir, store) = temp_store();
        store.ensure_builtin_installed().unwrap();
        store.set_enabled(super::BUILTIN_PACKAGE_ID, false).unwrap();

        // Mutate first, then prove calling this again does not clobber the
        // admin's own choice — a second install would silently re-enable
        // whatever this call flipped off.
        store.ensure_builtin_installed().unwrap();

        let packages = store.list().unwrap();
        assert_eq!(packages.len(), 1);
        assert_eq!(
            packages[0].status,
            PackageStatus::Disabled,
            "calling ensure_builtin_installed again must not re-enable a package an admin disabled"
        );
    }

    #[test]
    fn the_builtin_package_can_be_disabled_but_never_uninstalled() {
        let (_dir, store) = temp_store();
        store.ensure_builtin_installed().unwrap();

        // Mutate first: prove disabling actually works before proving
        // uninstall is refused, so a bug that broke both would not read as
        // "uninstall correctly refused".
        store.set_enabled(super::BUILTIN_PACKAGE_ID, false).unwrap();
        assert_eq!(
            store.list().unwrap()[0].status,
            PackageStatus::Disabled,
            "a built-in must still be disable-able"
        );
        store.set_enabled(super::BUILTIN_PACKAGE_ID, true).unwrap();

        let err = store.uninstall(super::BUILTIN_PACKAGE_ID).unwrap_err();
        assert!(
            matches!(err, WidgetPackageError::CannotUninstallBuiltIn(id) if id == super::BUILTIN_PACKAGE_ID)
        );
        assert_eq!(
            store.list().unwrap().len(),
            1,
            "a refused uninstall must leave the package exactly as it was"
        );
    }

    #[test]
    fn an_unknown_extension_point_inside_an_archive_is_rejected_by_name() {
        let (_dir, store) = temp_store();
        let manifest = r#"{
            "id": "acme-widgets",
            "name": "Acme",
            "version": "1.0.0",
            "contributes": [{ "point": "footer.banner" }]
        }"#;
        let archive = build_zip(&[("manifest.json", manifest.as_bytes())]);
        let err = store.install(&archive).unwrap_err();
        match err {
            WidgetPackageError::Manifest(super::ManifestError::UnknownExtensionPoint(point)) => {
                assert_eq!(point, "footer.banner");
            }
            other => panic!("expected UnknownExtensionPoint, got {other:?}"),
        }
    }
}
