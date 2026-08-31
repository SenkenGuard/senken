//! Only does anything when the `gui` feature is enabled: `tauri-build`
//! generates the Tauri context (icons, capabilities schema) that
//! `tauri::generate_context!()` in `src/gui.rs` expects. Without `gui`,
//! `tauri-build` is not even a dependency (see `Cargo.toml`), so this must
//! stay behind the same `cfg`.

fn main() {
    #[cfg(feature = "gui")]
    tauri_build::build();
}
