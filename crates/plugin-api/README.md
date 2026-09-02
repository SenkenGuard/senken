# senken-plugin-api

The public SDK for Senken plugins: everything a plugin author needs to
compile an indicator against Senken's plugin ABI, and nothing else.

- **One crate, generated from one WIT file.** `wit/senken.wit` — a sibling
  directory at the repository root, not inside this crate — is the single
  source of truth for what crosses the boundary between a plugin and the
  Senken host. This crate's guest bindings come from it via
  `wit_bindgen::generate!`; the host's own bindings, in `crates/plugin-host`,
  come from the exact same file via `wasmtime::component::bindgen!`. Neither
  side hand-edits what its generator produced — the same rule this
  repository already holds for `packages/web/src/lib/api/generated.ts`.
- **Depends on no Senken domain crate, ever.** `Cargo.toml` lists exactly
  two dependencies, `wit-bindgen` and `thiserror`, neither of which is a
  `senken-*` crate — enforced by a test that reads this crate's own
  manifest and fails the moment one appears. Once this SDK is published,
  a `senken-core` or `senken-series` dependency here would mean publishing
  Senken's internal implementation alongside the public contract, and
  every internal change to either would become a public break.
- **Prices and quantities cross as `(scale, value)`, never `f64`.** A
  `bar`'s `open`/`high`/`low`/`close`, its volume fields, and any
  `price-coord::executable` all carry their own scale explicitly, because a
  plugin has no other channel to learn what scale a raw integer is at.
  Indicator output (a `plot-point`'s `value`, a `price-coord::annotation`)
  is the one place `f64` is correct — it is a display or decision value,
  never an order price — matching the same exception the rest of Senken
  allows for indicator values and nowhere else.
- **A plugin can call back into the host's own ten built-in indicators.**
  `wit/senken.wit`'s `indicator-plugin` world imports `builtins` alongside
  exporting `indicator`, so `sma_update`/`ema_update`/`rsi_update` and their
  seven siblings — re-exported from this crate's root — call the same
  compiled, already-tested `senken_indicators` state machines the rest of
  Senken uses, rather than asking every plugin author to reimplement an
  EMA. `crates/indicator-lang`'s compiled output imports this exact same
  interface from this exact same file, so there is one definition of what
  calling a built-in means, not one per compiler.
- **The host↔domain conversion lives in [`convert`], as primitives, not
  types.** `convert::BarFields` mirrors `senken_series::Bar` field-for-field
  without depending on that crate; the runtime crate that already depends
  on both wires them together with a direct field-by-field call. The
  round-trip is tested here, at an uncommon real venue scale (BitMart's own
  reported 12-digit spot price precision), so the property is proven once
  and reused rather than re-verified at every call site.
- **Versioned on its own**, starting at `0.1.0`, independent of the Senken
  application's `version.workspace`. The WIT package itself carries the
  same number (`senken:plugin-api@0.1.0`), which becomes part of a compiled
  component's type — a host can read which SDK version a plugin was built
  against without calling into it at all.
- **Licensed `MIT OR Apache-2.0`**, not the application's `LGPL-3.0-only`:
  this is the crate a plugin author's own code links against, and that pair
  is what the Rust ecosystem expects a crate in that position to offer.
- **Out of scope here**: loading, sandboxing, or running a compiled plugin
  (`crates/plugin-host`); the indicator language compiler
  (`crates/indicator-lang`); anything about a venue-adapter world — this
  first world covers indicators only, which is what needs no host-mediated
  I/O and so proves the ABI shape first.
