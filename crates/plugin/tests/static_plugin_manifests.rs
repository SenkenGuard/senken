//! Validates every static plugin's checked-in `plugins/<id>/senken-plugin.json`
//! by actually reading it off disk and running it through the same parser
//! (`senken_plugin::parse_static_contributions`) each plugin crate's own
//! `manifest()` feeds it via `include_str!` — so a typo in one of these
//! files is a failing `cargo test` here, not a panic the first time
//! something calls that plugin's `manifest()` at runtime (every venue crate
//! does exactly `parse_static_contributions(include_str!("../senken-plugin.json")).expect(...)`,
//! which is a panic, not a `Result`, on bad JSON).
//!
//! `plugins/widgets/` is not iterated: it holds package-shaped widget UI
//! plugins (a `manifest.json` plus a `web/` bundle, no `senken-plugin.json`
//! at all), validated by its own `crates/plugin/tests/example_widget_plugins.rs`
//! against `senken_plugin::widget_package::manifest::validate` instead.

use std::path::{Path, PathBuf};

use senken_plugin::parse_static_contributions;
use serde::Deserialize;

/// The repository's `plugins/` directory, resolved from this crate's own
/// manifest directory so the test works regardless of the caller's current
/// directory.
fn plugins_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins")
}

/// Just enough of a static plugin's `senken-plugin.json` to check the
/// fields `parse_static_contributions` itself does not — that parser only
/// ever reads `contributes`, by design (see its own doc comment on why it
/// asks for less than the package-manifest validator does).
#[derive(Deserialize)]
struct StaticManifestFields {
    schema_version: u32,
    id: String,
    name: String,
    version: String,
}

/// One checked-in manifest, read and validated — panics with the offending
/// path on any failure, so a broken manifest names itself in the test
/// output rather than reporting a bare assertion failure.
fn validate_manifest(dir_name: &str, path: &Path) {
    let raw =
        std::fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()));

    let fields: StaticManifestFields = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("{} is not a valid manifest: {e}", path.display()));
    assert_eq!(
        fields.schema_version,
        1,
        "{}: unrecognized schema_version",
        path.display()
    );
    assert_eq!(
        fields.id,
        dir_name,
        "{}: manifest id must match its own directory name",
        path.display()
    );
    assert!(
        !fields.name.trim().is_empty(),
        "{}: name must not be empty",
        path.display()
    );
    assert!(
        !fields.version.trim().is_empty(),
        "{}: version must not be empty",
        path.display()
    );

    let contributes =
        parse_static_contributions(&raw).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    assert!(
        !contributes.is_empty(),
        "{}: a plugin must declare at least one contribution",
        path.display()
    );
}

#[test]
fn every_static_plugins_manifest_on_disk_is_well_formed_and_matches_its_directory() {
    let root = plugins_root();
    let entries =
        std::fs::read_dir(&root).unwrap_or_else(|e| panic!("reading {}: {e}", root.display()));

    let mut checked = 0usize;
    for entry in entries {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        let dir_name = entry.file_name().into_string().unwrap();
        // `widgets/` is package-shaped, not a static plugin — see this
        // file's own module doc comment.
        if dir_name == "widgets" {
            continue;
        }
        let manifest_path = entry.path().join("senken-plugin.json");
        assert!(
            manifest_path.is_file(),
            "{} has no senken-plugin.json — every static plugin crate must ship one",
            entry.path().display()
        );
        validate_manifest(&dir_name, &manifest_path);
        checked += 1;
    }

    // A floor, not a magic number: if this collapses to zero, the
    // directory walk broke silently (a wrong path, an early `continue`)
    // and every check above passed for having nothing to check — the
    // exact failure mode this test exists to rule out for the manifests
    // themselves.
    assert!(
        checked >= 20,
        "expected to validate at least 20 static plugin manifests under {}, found {checked}",
        root.display()
    );
}
