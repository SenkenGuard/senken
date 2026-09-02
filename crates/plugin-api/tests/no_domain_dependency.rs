//! Proves the boundary this crate's own `Cargo.toml` states in prose:
//! `senken-plugin-api` must never depend on a Senken domain crate.
//!
//! Publishing this SDK with a `senken-core` or `senken-series` dependency
//! would publish that crate's implementation alongside it, and every
//! internal change to either would then become a public break. This is
//! enforced here rather than left to be remembered, by reading the actual
//! manifest rather than trusting anyone's memory of what it says.

/// `true` for a table header naming a dependency section — `dependencies`,
/// `dev-dependencies`, `build-dependencies`, or one of the equivalent
/// target-specific tables (`[target.'cfg(...)'.dependencies]` and so on).
fn is_dependency_section(section: &str) -> bool {
    matches!(
        section,
        "dependencies" | "dev-dependencies" | "build-dependencies"
    ) || section.ends_with(".dependencies")
        || section.ends_with(".dev-dependencies")
        || section.ends_with(".build-dependencies")
}

/// Every dependency key (the left-hand side of a manifest line inside a
/// dependency table) found in `manifest`.
fn dependency_keys(manifest: &str) -> Vec<&str> {
    let mut section = String::new();
    let mut keys = Vec::new();
    for raw_line in manifest.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix('[') {
            rest.trim_end_matches(']').clone_into(&mut section);
            continue;
        }
        if !is_dependency_section(&section) {
            continue;
        }
        if let Some((key, _)) = line.split_once('=') {
            keys.push(key.trim().trim_matches('"'));
        }
    }
    keys
}

#[test]
fn manifest_declares_no_senken_domain_dependency() {
    let manifest = include_str!("../Cargo.toml");
    for key in dependency_keys(manifest) {
        assert!(
            key == "senken-plugin-api" || !key.starts_with("senken-"),
            "senken-plugin-api must never depend on a Senken domain crate — \
             found {key:?} in Cargo.toml. Publishing this SDK with that \
             dependency would publish that crate's implementation too."
        );
    }
}

/// Not itself part of the guard — a fixture proving [`dependency_keys`]
/// actually finds what it claims to, on manifest text this test controls
/// directly rather than the real (currently clean) `Cargo.toml`.
#[test]
fn dependency_keys_finds_entries_in_every_section_shape_it_claims_to_cover() {
    let manifest = r#"
[package]
name = "not-a-real-crate"

[dependencies]
wit-bindgen = "0.57"

[dev-dependencies]
wasmtime = { version = "48" }

[target.'cfg(unix)'.dependencies]
some-unix-only-crate = "1.0"
"#;
    let keys = dependency_keys(manifest);
    assert_eq!(
        keys,
        vec!["wit-bindgen", "wasmtime", "some-unix-only-crate"]
    );
}
