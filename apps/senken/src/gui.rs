//! The desktop window, behind the `gui` feature only.
//!
//! Tauri is used purely as a library here: there is no `cargo tauri`
//! scaffolding, no sidecar and no second binary. The window loads the
//! embedded web app through the same local server `senken serve` uses —
//! never a separate asset pipeline — so the browser and desktop paths
//! render byte-identical UI.

use std::net::SocketAddr;

use anyhow::Context as _;

use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

/// Opens the desktop window pointed at `addr` and blocks until it closes.
///
/// `runtime_handle` lets Tauri's internal async plumbing schedule work on
/// the Tokio runtime the caller is already running on, instead of spinning
/// up a second one.
pub(crate) fn run_window(
    runtime_handle: tokio::runtime::Handle,
    addr: SocketAddr,
    ui: Option<&str>,
) -> anyhow::Result<()> {
    tauri::async_runtime::set(runtime_handle);

    // `ui` exists for development: pointing the window at a Vite dev server
    // gives the desktop build the same hot reload the browser gets. The
    // embedded server still runs, and the dev server proxies `/api` back to
    // it, so both surfaces talk to one backend either way.
    let url: tauri::Url = match ui {
        Some(ui) => ui
            .parse()
            .with_context(|| format!("`--ui {ui}` is not a valid URL"))?,
        None => format!("http://{addr}")
            .parse()
            .expect("a `SocketAddr` always formats into a valid URL"),
    };

    tauri::Builder::default()
        .setup(move |app| {
            let builder = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url.clone()))
                .title("Senken")
                .inner_size(1280.0, 800.0)
                .initialization_script(SHELL_MARKER);

            build_chromeless(builder).build()?;
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                // The last window closing ends the run loop below; the
                // caller then shuts the embedded server down and exits.
                window.app_handle().exit(0);
            }
        })
        .run(tauri::generate_context!())
        .map_err(anyhow::Error::from)
}

/// Marks the document as running inside the desktop shell, and says which
/// platform's window chrome it has to accommodate.
///
/// The same HTML serves the browser and this window, so the stylesheet cannot
/// infer either fact. A marker on the root element lets CSS handle it with no
/// branching in the app itself — a plain browser simply never sees the
/// attribute and lays out normally.
const SHELL_MARKER: &str = if cfg!(target_os = "macos") {
    "document.documentElement.dataset.shell = 'tauri-macos';"
} else {
    "document.documentElement.dataset.shell = 'tauri';"
};

/// Removes the native title bar so the page can draw its own, in whichever
/// way the platform actually supports.
///
/// The two platforms need opposite things, which is why this is not one flag:
///
/// - **macOS** keeps its frame and only makes the title bar transparent, so
///   the traffic lights stay *native* — right position, right behaviour, right
///   full-screen animation. Nothing here can reproduce those faithfully, and
///   users would notice. The cost is that the page must leave room for them,
///   which the `tauri-macos` marker drives.
/// - **Windows and Linux** have no such mode: the frame goes entirely, and the
///   page is then responsible for minimise, maximise and close.
#[cfg(target_os = "macos")]
fn build_chromeless<R: tauri::Runtime, M: tauri::Manager<R>>(
    builder: WebviewWindowBuilder<'_, R, M>,
) -> WebviewWindowBuilder<'_, R, M> {
    builder
        .title_bar_style(tauri::utils::TitleBarStyle::Overlay)
        .hidden_title(true)
}

// The traffic lights are deliberately left where macOS puts them.
//
// `traffic_light_position` exists and was tried, but its y origin is not the
// window's top edge, so forcing a value put the lights hard against the frame.
// The system's own placement is correct by construction and matches every
// other Mac app; the page's title-bar strip is sized to suit it instead
// (`--shell-title-h` in packages/web/src/routes/layout.css).

/// Windows and Linux **keep** their native frame, deliberately, for now.
///
/// `decorations(false)` is the call that would remove it, and it is one line.
/// What stops us is the consequence: with no frame, the page owes the user
/// minimise, maximise and close — and those need Tauri IPC, which this window
/// cannot use. It loads an *external* URL (the local server), and IPC to a
/// remote origin requires a `remote` entry in `capabilities/default.json` that
/// this project has not added or verified.
///
/// Shipping a frameless Windows window whose only way out is Alt+F4 would be
/// worse than an ordinary title bar. macOS has no such problem because its
/// traffic lights stay native under `Overlay`.
///
/// To finish this: add the local origin to the capability's `remote.urls`,
/// implement the three controls, and verify on a real Windows machine —
/// including that Snap Layouts and the resize edges still behave, neither of
/// which survives frame removal for free.
#[cfg(not(target_os = "macos"))]
fn build_chromeless<R: tauri::Runtime, M: tauri::Manager<R>>(
    builder: WebviewWindowBuilder<'_, R, M>,
) -> WebviewWindowBuilder<'_, R, M> {
    builder
}
