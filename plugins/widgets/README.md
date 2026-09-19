# Example widget UI plugins

Two independent, minimal widget plugin packages, each built the way a real
third-party plugin author would build one: a `manifest.json` plus a `web/`
directory of static assets, no Svelte, no Tailwind, no build step, and no
dependency on anything in this repository's own frontend. Either directory
here is exactly what gets zipped up and installed.

- `example-clock/` — a live clock (`dataSource: "live"`), with a config
  value (`hour12`) the widget itself reads and writes through the host, to
  exercise `config.get`/`config.patch` round-tripping.
- `example-quotes/` — a rotating fixture quote (`dataSource: "mock"`), whose
  own markup deliberately (and misleadingly) renders a "Live feed" badge —
  see the comment in its `web/index.html`. Install it and open the
  dashboard: the host still draws its own, honest "Mock" label above the
  iframe, because that label comes from the manifest the host already
  validated, never from anything the widget renders or claims. A widget has
  no way to suppress or fake it.

`example-clock` is also compiled straight into the server binary
(`crates/plugin/src/widget_package/store.rs`'s `BUILTIN_MANIFEST`/
`BUILTIN_INDEX_HTML`, via `include_str!` on the two files here — never a
copy) and installed automatically on every fresh start, so Settings →
Plugins shows a real, working plugin from the first run rather than an
empty list. `example-quotes` is deliberately left out of that: its zip
stays sitting here so there is still something to try the upload flow
with on a fresh install.

## Packaging

The install endpoint (and the same-shaped file-drop path) takes a zip
archive with `manifest.json` at its root and the widget's assets under
`web/`. `example-clock.zip` and `example-quotes.zip`, right next to their
source directories in this folder, are exactly that archive, already
built — upload one directly, no command needed.

To rebuild one after editing its source (or to package a new example the
same way), from either example's own directory:

```sh
cd example-clock   # or example-quotes
zip -r ../example-clock.zip manifest.json web
```

No other tooling is involved — that archive is the complete, installable
artifact. `crates/plugin/tests/example_widget_plugins.rs` installs the
checked-in zip through the real package store on every test run, so a zip
left stale after an edit to its source is caught there rather than on
someone's first upload.

## Installing

Either upload the resulting `.zip` through Settings → Plugins → Widget
plugins → Install, or drop it, already unzipped into its own directory
named for the package's own `id` (`example-clock/`, `example-quotes/`),
directly under this server's `widget-plugins/packages/` data directory and
use that same page's "Refresh" button — both paths converge on the same
on-disk layout, and neither executes anything: the archive is data until
the host's own manifest validator and path-safety checks pass.

## The Content-Security-Policy a widget's own markup must fit

The sandboxed iframe a widget's `index.html` loads into has no
`allow-same-origin`, which makes its document's origin opaque — and the
`'self'` CSP keyword cannot match anything for an opaque origin. To still
let a widget's own inline code run at all, the host computes a
`'sha256-...'` CSP source from the exact text of every **bare** `<script>`
and `<style>` tag in the served document (no attributes on the tag itself —
`<script type="module">` or `<script src="...">` do not qualify) and allows
only those. Both examples here are written this way on purpose: one
`<style>` block, one `<script>` block, neither with any attribute. A widget
that needs a second script or stylesheet file, rather than more markup
inside the one bare `<script>`/`<style>` tag it already has, is not served
by this policy yet — put everything in the one document, and ship any
image or font as a `data:` URI, until this widget's entry document is
served from a genuinely separate origin (`crates/api/src/widget_plugin_handlers.rs`'s
own doc comment on `widget_plugin_asset` names this as a follow-up).
