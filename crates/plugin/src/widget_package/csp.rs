//! The Content-Security-Policy header for a dynamic widget's entry
//! document — the one piece of this sandbox that must be computed from the
//! document's own bytes rather than written down as one fixed string.
//!
//! # Why a fixed CSP does not work here
//!
//! `plugin-widget-frame.svelte` hosts a widget's document in
//! `<iframe sandbox="allow-scripts">` with **no** `allow-same-origin` — on
//! purpose, since that is this platform's real isolation boundary (see that
//! component's own doc comment). The cost of that choice is that the
//! document's origin is **opaque**, and the `'self'` CSP keyword resolves
//! against a document's origin. An opaque origin is never equal to
//! anything, including a second read of itself, so `'self'` cannot match
//! *any* request the document makes — not its own inline `<script>`, not an
//! external file it would otherwise be allowed to load from the exact host
//! that served it. A CSP built the ordinary way (`script-src 'self'`) is
//! therefore not a stricter policy for this iframe; it is a policy that
//! blocks the widget outright, which is exactly the "white box" bug this
//! module exists to fix.
//!
//! # Why this uses a hash source, not a nonce or an explicit origin
//!
//! Two other fixes were weighed and rejected:
//!
//! - **A nonce per response** (`script-src 'nonce-<random>'`) is the fix
//!   VS Code's own webviews use, and it works on an opaque origin the same
//!   way a hash does — but only because a webview's HTML is a template its
//!   *host* builds, so the host can write the matching `nonce="..."`
//!   attribute into the very tag it is templating. A widget bundle here is
//!   the opposite: an already-built, static file a plugin author zipped up
//!   with no build step at all (see `examples/widget-plugins/README.md`).
//!   Making a nonce match would mean this server rewriting a third party's
//!   markup on every response to splice an attribute into whatever
//!   `<script>`/`<style>` tag it finds — a second, home-grown HTML mutation
//!   step with its own correctness surface (nested quotes, an existing
//!   `nonce` attribute, a tag name appearing inside the script's own text).
//!   A hash needs none of that: it is computed by *reading* the exact bytes
//!   already on disk, never by editing them.
//! - **Naming this server's own origin explicitly** (`script-src
//!   http://<host>:<port>`) would restore `'self'`-like matching for a
//!   *fetched* resource (host-source matching compares to the request URL,
//!   not to the document's opaque origin, so it is not affected by the same
//!   bug) — but it does nothing for an *inline* `<script>` at all: no
//!   source-list keyword or origin ever authorises inline content, opaque
//!   origin or not. It also cannot be derived safely here: this crate does
//!   not know the scheme/host a browser will actually use to reach the
//!   server (`ServeOptions` names a bind address, not a public origin, and
//!   a dev proxy may front it at a different one entirely — see
//!   `packages/web/vite.config.ts`), and building it from a request's own
//!   `Host` header would mean trusting a client-supplied value inside a
//!   security header.
//!
//! A hash source sidesteps both problems for the shape every widget bundle
//! actually has today: a single static document with its script and style
//! inlined directly into it (`examples/widget-plugins/example-clock` and
//! `example-quotes` both look exactly like this). [`content_security_policy`]
//! reads the exact inline content HTML5 itself defines for a `<script>` or
//! `<style>` element — see this module's own private `raw_text_contents`
//! helper — hashes it, and allows nothing else to run.
//!
//! # What this does not yet cover
//!
//! A widget that ships a *separate* `widget.js` or `style.css` file
//! (`<script src="widget.js">`, `<link rel="stylesheet" href="style.css">`)
//! is not served by this policy: hash sources only ever authorise inline
//! content, and host-source origin matching is the piece named above that
//! this crate cannot safely compute yet. Until this widget's entry document
//! is served from a genuinely separate origin (`widget_plugin_asset`'s own
//! doc comment already names this as a follow-up), a plugin that wants more
//! than one file's worth of code or style should inline it, or ship
//! additional images/fonts as `data:` URIs — exactly what `img-src`'s own
//! `data:` allowance already exists for.

use sha2::{Digest, Sha256};

/// The exact Content-Security-Policy this platform serves with every
/// dynamic widget's entry document. See this module's own docs for why it
/// is built from `html` rather than written down as one fixed string.
///
/// `connect-src 'none'` is the one piece that never changes no matter what
/// `html` contains: every widget-to-host call goes through
/// `postMessage`, never a network request the widget makes for itself, and
/// this line is what makes that a rule rather than a convention.
#[must_use]
pub fn content_security_policy(html: &str) -> String {
    let script_sources = hash_sources(html, "script");
    let style_sources = hash_sources(html, "style");

    let mut policy = String::from("default-src 'none'; ");
    // An empty source list is left out entirely rather than emitted as
    // (say) `script-src;` — omitting the directive falls back to
    // `default-src 'none'` above, which blocks the same thing a
    // deliberately-empty list would, without a directive whose own syntax
    // looks like a mistake to the next reader.
    if !script_sources.is_empty() {
        policy.push_str("script-src ");
        policy.push_str(&script_sources.join(" "));
        policy.push_str("; ");
    }
    if !style_sources.is_empty() {
        policy.push_str("style-src ");
        policy.push_str(&style_sources.join(" "));
        policy.push_str("; ");
    }
    // `'self'` is kept here even though it cannot match anything for this
    // opaque-origin document today (see this module's own docs) — an image
    // or a font has no hash-source equivalent to fall back to, and `'self'`
    // is the one expression that starts matching for free the day this
    // widget's entry document is served from a genuine second origin
    // instead of a sandboxed same-origin iframe. Until then, `data:` is the
    // path that actually works, for both.
    policy.push_str("img-src 'self' data:; connect-src 'none'; font-src 'self' data:; ");
    policy.push_str("object-src 'none'; base-uri 'none'; form-action 'none';");
    policy
}

/// Every `'sha256-<base64>'` CSP source for each bare, attribute-free
/// `<tag>...</tag>` this document contains, in the order they appear.
///
/// Deliberately narrow: this matches only the exact `<script>`/`<style>`
/// form `examples/widget-plugins/README.md` tells a plugin author to write
/// (no build tooling, no attributes). A tag with an attribute — `<script
/// type="module">`, `<script src="...">` — is not matched at all, so it
/// contributes no hash and the browser blocks it, which is the safe
/// direction for a case this function does not understand rather than a
/// guess at one.
fn hash_sources(html: &str, tag: &'static str) -> Vec<String> {
    raw_text_contents(html, tag)
        .map(|content| {
            let digest = Sha256::digest(content.as_bytes());
            format!(
                "'sha256-{}'",
                base64::Engine::encode(&base64::engine::general_purpose::STANDARD, digest)
            )
        })
        .collect()
}

/// Yields the exact text HTML5 itself would hand a `<script>` or `<style>`
/// element as its content, for every bare `<tag>...</tag>` occurrence in
/// `html`.
///
/// This is a search for two literal byte strings, not a general HTML
/// parser — and that is enough to be *exact*, not merely approximate,
/// precisely because `<script>` and `<style>` are HTML5's own "raw text"
/// elements: the spec defines their content as ending at the first
/// `</tag>` sequence, full stop, with no nested-tag awareness at all (this
/// is the same rule that makes `document.write("<\/script>")`'s escape
/// necessary in hand-written JS — a literal `</script>` inside the text
/// would otherwise end the element early). Searching for the first
/// `</tag>` after each `<tag>` therefore finds exactly the same boundary a
/// browser's own tokenizer would, so the bytes hashed here are exactly the
/// bytes a browser hashes when it checks this policy's `'sha256-...'`
/// source — no separate parser is needed for that guarantee to hold.
fn raw_text_contents<'html>(
    html: &'html str,
    tag: &'static str,
) -> impl Iterator<Item = &'html str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let lower = html.to_ascii_lowercase();
    let mut cursor = 0usize;
    std::iter::from_fn(move || {
        let open_at = lower[cursor..].find(&open)? + cursor;
        let content_start = open_at + open.len();
        let close_at = lower[content_start..].find(&close)? + content_start;
        cursor = close_at + close.len();
        Some(&html[content_start..close_at])
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_a_bare_inline_script_and_leaves_out_style_when_there_is_none() {
        let html = "<html><body><script>console.log('hi');</script></body></html>";
        let policy = content_security_policy(html);
        assert!(
            policy.contains("script-src 'sha256-"),
            "expected a script-src hash source, got: {policy}"
        );
        assert!(
            !policy.contains("style-src"),
            "no <style> tag exists, so style-src must be left out entirely: {policy}"
        );
        assert!(policy.starts_with("default-src 'none';"));
        assert!(policy.contains("connect-src 'none'"));
    }

    #[test]
    fn the_hash_matches_hand_computed_sha256_of_the_exact_script_text() {
        let script_text = "const x = 1;";
        let html = format!("<script>{script_text}</script>");
        let policy = content_security_policy(&html);

        let expected = base64::Engine::encode(
            &base64::engine::general_purpose::STANDARD,
            Sha256::digest(script_text.as_bytes()),
        );
        assert!(
            policy.contains(&format!("'sha256-{expected}'")),
            "policy {policy} did not contain the expected hash of {script_text:?}"
        );
    }

    #[test]
    fn a_script_tag_with_an_attribute_is_not_matched() {
        // `<script type="module">` is not the bare form this function
        // understands (see its own doc comment) — it must contribute no
        // hash, leaving the browser to block it rather than this function
        // guessing at a boundary it cannot be sure of.
        let html = "<script type=\"module\">console.log('hi');</script>";
        let policy = content_security_policy(html);
        assert!(
            !policy.contains("script-src"),
            "an attributed <script> tag must not be treated as the bare form: {policy}"
        );
    }

    #[test]
    fn two_inline_scripts_each_get_their_own_hash_source() {
        let html = "<script>const a = 1;</script><p>x</p><script>const b = 2;</script>";
        let policy = content_security_policy(html);
        let hash_count = policy.matches("'sha256-").count();
        assert_eq!(
            hash_count, 2,
            "expected one hash per inline <script>, got policy: {policy}"
        );
    }

    #[test]
    fn a_script_with_no_closing_tag_contributes_nothing() {
        let html = "<script>console.log('unterminated');";
        let policy = content_security_policy(html);
        assert!(
            !policy.contains("script-src"),
            "an unterminated <script> must not produce a hash for content that never actually \
             closed: {policy}"
        );
    }

    #[test]
    fn connect_src_is_always_none_regardless_of_content() {
        // The one line this module's own docs promise never changes.
        for html in ["<script>x</script>", "<p>no script or style at all</p>", ""] {
            assert!(content_security_policy(html).contains("connect-src 'none'"));
        }
    }
}
