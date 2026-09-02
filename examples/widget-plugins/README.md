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

## Packaging

The install endpoint (and the same-shaped file-drop path) takes a zip
archive with `manifest.json` at its root and the widget's assets under
`web/`. From either example's own directory:

```sh
cd example-clock   # or example-quotes
zip -r ../example-clock.zip manifest.json web
```

No other tooling is involved — that archive is the complete, installable
artifact.

## Installing

Either upload the resulting `.zip` through the dashboard's own "Widget
plugins" manager (workspace bar → "…" → "Widget plugins…" → Install), or
drop it, already unzipped into its own directory named for the package's
own `id` (`example-clock/`, `example-quotes/`), directly under this
server's `widget-plugins/packages/` data directory and use that same
dialog's "Refresh" button — both paths converge on the same on-disk layout,
and neither executes anything: the archive is data until the host's own
manifest validator and path-safety checks pass.
