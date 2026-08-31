//! Serves the embedded `SvelteKit` build with an SPA fallback.
//!
//! `rust-embed` embeds `packages/web/build` at compile time in release, and reads it
//! straight off disk in debug — either way `cargo check` must
//! succeed even when the directory holds nothing but a placeholder, which is
//! why `packages/web/build/.gitkeep` is tracked in git instead of the whole directory
//! being gitignored.

use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "$CARGO_MANIFEST_DIR/../../packages/web/build"]
struct WebAssets;

const NOT_BUILT: &str = "web assets not built — run `bun run build:web` from the repository root";

/// Serves `uri` from the embedded build, falling back to `index.html` for
/// any path that does not match a built asset — the piece that lets
/// client-side routing survive a hard refresh.
pub(crate) async fn fallback(uri: Uri) -> Response {
    let requested = uri.path().trim_start_matches('/');
    let path = if requested.is_empty() {
        "index.html"
    } else {
        requested
    };

    if let Some(file) = WebAssets::get(path) {
        return serve(path, file.data);
    }

    match WebAssets::get("index.html") {
        Some(file) => serve("index.html", file.data),
        None => (StatusCode::NOT_FOUND, NOT_BUILT).into_response(),
    }
}

fn serve(path: &str, data: std::borrow::Cow<'static, [u8]>) -> Response {
    ([(header::CONTENT_TYPE, content_type_for(path))], data).into_response()
}

/// A small, explicit extension-to-MIME-type table rather than pulling in a
/// guessing crate: a `SvelteKit` static build only ever emits this fixed set
/// of file types.
fn content_type_for(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or_default() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "ico" => "image/x-icon",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "wasm" => "application/wasm",
        "txt" => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::content_type_for;

    #[test]
    fn known_extensions_map_to_their_mime_type() {
        assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
        assert_eq!(
            content_type_for("_app/immutable/entry.js"),
            "text/javascript; charset=utf-8"
        );
        assert_eq!(content_type_for("favicon.svg"), "image/svg+xml");
    }

    #[test]
    fn an_unknown_extension_falls_back_to_octet_stream() {
        assert_eq!(content_type_for("data.bin"), "application/octet-stream");
        assert_eq!(content_type_for("no-extension"), "application/octet-stream");
    }
}
