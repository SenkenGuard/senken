//! Proves the example widget plugins under `examples/widget-plugins/` in
//! the repository root are not merely plausible-looking — each one is
//! zipped up exactly the way `examples/widget-plugins/README.md` tells a
//! real plugin author to do it, and installed through the real
//! [`WidgetPackageStore`], the same entry point `crates/api`'s widget
//! plugin upload handler calls. If either example ever drifts out of sync
//! with the manifest schema this crate actually validates, this is where
//! that shows up — not as a runtime surprise the first time someone
//! actually tries to install one.

use std::io::Write;
use std::path::{Path, PathBuf};

use senken_plugin::widget_package::{DataSource, PackageStatus, WidgetPackageStore};
use tempfile::TempDir;
use zip::write::SimpleFileOptions;

/// `examples/widget-plugins/<name>`, resolved from this crate's own
/// manifest directory so the test works regardless of the caller's current
/// directory.
fn example_dir(name: &str) -> PathBuf {
    examples_root().join(name)
}

/// `examples/widget-plugins/`, resolved from this crate's own manifest
/// directory.
fn examples_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/widget-plugins")
}

/// Zips exactly `manifest.json` and `web/index.html` out of `dir` — the
/// same two files `examples/widget-plugins/README.md`'s packaging
/// instructions produce for either example today. A future example with
/// more assets would extend this list, not switch to a generic recursive
/// walk, so this test keeps naming exactly what it expects to find rather
/// than trusting whatever happens to be on disk.
fn zip_example(dir: &Path) -> Vec<u8> {
    let manifest = std::fs::read(dir.join("manifest.json"))
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.join("manifest.json").display()));
    let index_html = std::fs::read(dir.join("web/index.html"))
        .unwrap_or_else(|e| panic!("reading {}: {e}", dir.join("web/index.html").display()));

    let mut buffer = std::io::Cursor::new(Vec::new());
    {
        let mut writer = zip::ZipWriter::new(&mut buffer);
        let options =
            SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file("manifest.json", options).unwrap();
        writer.write_all(&manifest).unwrap();
        writer.start_file("web/index.html", options).unwrap();
        writer.write_all(&index_html).unwrap();
        writer.finish().unwrap();
    }
    buffer.into_inner()
}

#[test]
fn the_example_clock_plugin_installs_and_contributes_a_live_widget() {
    let dir = TempDir::new().unwrap();
    let store = WidgetPackageStore::open(dir.path()).unwrap();

    let archive = zip_example(&example_dir("example-clock"));
    let id = store
        .install(&archive)
        .expect("the packaged example-clock archive must pass real manifest validation");
    assert_eq!(id, "example-clock");

    let packages = store.list().unwrap();
    assert_eq!(packages.len(), 1);
    assert_eq!(packages[0].status, PackageStatus::Active);
    assert_eq!(packages[0].widgets.len(), 1);
    let widget = &packages[0].widgets[0];
    assert_eq!(widget.widget_type_id, "example-clock/clock");
    assert_eq!(widget.data_source, DataSource::Live);
    assert_eq!(widget.entry, "index.html");

    // The asset the manifest names as its entry must actually be
    // resolvable and readable back out of the installed package.
    let asset = store
        .resolve_asset("example-clock", "index.html")
        .unwrap()
        .expect("the entry file must resolve for an active package");
    let served = std::fs::read_to_string(asset).unwrap();
    assert!(
        served.contains("senken.widget"),
        "must speak the widget host protocol"
    );
}

#[test]
fn the_example_quotes_plugin_installs_and_declares_itself_mock() {
    let dir = TempDir::new().unwrap();
    let store = WidgetPackageStore::open(dir.path()).unwrap();

    let archive = zip_example(&example_dir("example-quotes"));
    let id = store
        .install(&archive)
        .expect("the packaged example-quotes archive must pass real manifest validation");
    assert_eq!(id, "example-quotes");

    let catalog = store.effective_widget_catalog().unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].widget_type_id, "example-quotes/daily-quote");
    assert_eq!(
        catalog[0].data_source,
        DataSource::Mock,
        "this example's whole point is a plugin honestly declaring mock data \
         even though its own markup renders a misleading 'Live feed' badge — \
         the host's mockup label comes from this field, never from the \
         widget's own rendered output"
    );
}

/// `examples/widget-plugins/README.md` also ships a pre-built `<name>.zip`
/// next to each example, so trying the upload flow needs no `zip` command
/// at all. This proves that checked-in archive installs for real, through
/// the same [`WidgetPackageStore`] the upload handler uses — if an editor
/// changes `manifest.json` or `web/index.html` without rebuilding the zip,
/// this is where that drift shows up, rather than a user's first upload
/// silently installing stale content.
fn assert_prebuilt_zip_installs(name: &str, expected_widget_type_id: &str) {
    let dir = TempDir::new().unwrap();
    let store = WidgetPackageStore::open(dir.path()).unwrap();

    let archive_path = examples_root().join(format!("{name}.zip"));
    let archive = std::fs::read(&archive_path)
        .unwrap_or_else(|e| panic!("reading {}: {e}", archive_path.display()));

    let id = store
        .install(&archive)
        .expect("the checked-in prebuilt zip must pass real manifest validation");
    assert_eq!(id, name);

    let catalog = store.effective_widget_catalog().unwrap();
    assert_eq!(catalog.len(), 1);
    assert_eq!(catalog[0].widget_type_id, expected_widget_type_id);
}

#[test]
fn the_prebuilt_example_clock_zip_matches_its_source_and_installs() {
    assert_prebuilt_zip_installs("example-clock", "example-clock/clock");
}

#[test]
fn the_prebuilt_example_quotes_zip_matches_its_source_and_installs() {
    assert_prebuilt_zip_installs("example-quotes", "example-quotes/daily-quote");
}

#[test]
fn both_examples_can_be_installed_side_by_side_and_disabling_one_leaves_the_other_active() {
    let dir = TempDir::new().unwrap();
    let store = WidgetPackageStore::open(dir.path()).unwrap();

    store
        .install(&zip_example(&example_dir("example-clock")))
        .unwrap();
    store
        .install(&zip_example(&example_dir("example-quotes")))
        .unwrap();

    let catalog = store.effective_widget_catalog().unwrap();
    let mut widget_type_ids: Vec<_> = catalog.iter().map(|w| w.widget_type_id.clone()).collect();
    widget_type_ids.sort();
    assert_eq!(
        widget_type_ids,
        vec!["example-clock/clock", "example-quotes/daily-quote"]
    );

    store.set_enabled("example-clock", false).unwrap();
    let catalog = store.effective_widget_catalog().unwrap();
    assert_eq!(
        catalog.len(),
        1,
        "disabling one plugin must not affect the other"
    );
    assert_eq!(catalog[0].widget_type_id, "example-quotes/daily-quote");
}
