# Porting a venue to a `venue-plugin` wasm component

Checklist from OKX (first) and Bybit (second, ~1 hour hands-on — most of
the fixed cost was OKX paying for the SDK/build script/test harness once).
Steps, exact commands, and the traps actually hit — not a plan, a record.

1. **Extract `<venue>-core`** (`plugins/<venue>/core/`, deps: `senken-core`,
   `serde`, `serde_json`, `thiserror`). `wit/senken.wit`'s `instrument`
   record now carries `kind`/`status`/`contract` (settle, settlement,
   expiry, contract size, option strike/right), so a derivative market can
   be ported too — see step 9 below for OKX's swap/futures/two option
   families, the first venue to do it. A market with no recorded fixture
   yet, or with a shape core has not been extended to parse (a new
   settlement currency, say), still stays native rather than guessing.
   Copy, don't import, from `senken-venue`/
   `senken-marketdata` (`Num`, `common_scale`, `normalise_symbol`) — they
   pull `reqwest`/`tokio`, won't compile for `wasm32-wasip2`. Trap: if the
   native normaliser is a *generic* call (Bybit:
   `senken_venue::normalise_symbol(s, &['-'])`) rather than its own
   function (OKX's shape), give core one anyway and repoint **both**
   native call sites (catalog, live decoder) — the generic call is not
   the "one function" `AGENTS.md` needs. Trap: a closure rule that isn't a
   per-row `confirm` flag (Bybit: server `time` + the spec's duration)
   needs its own `duration_nanos` in core — no `senken-series` in a guest.
2. `cargo test -p senken-plugin-<venue>` green, zero fixture changes.
3. **`<venue>-venue` wasm crate** (own `[workspace]`, `crate-type =
   ["cdylib"]`, deps `senken-plugin-api` + `<venue>-core`). Implement
   `VenueGuest`, field-copy core types to WIT types. Build:
   `plugins/build-venue.sh <venue>`.
4. **`tests/wasm_parity.rs`** + `tests/support/mod.rs` (copy OKX's). One
   `wiremock` server, both sides, compare field-by-field. Trap: no
   suspended row in the fixture (Bybit's `spot.json` has one row, Trading)
   means the catalog-omission property needs a *synthetic* JSON snippet in
   core's own unit tests instead (same precedent as Bybit's pre-existing
   `a_cursor_is_read_only_…` test) — never a hand-written full fixture.
5. **`senken-plugin.json` gets `entry`/`base_url`; `Plugin::venue_components()`
   returns a `Vec` of `include_bytes!("../dist/…wasm")`; drop that
   market's native `register_marketdata_source`/`register_bar_source`
   calls.** A venue with only one source id to preserve returns a
   one-element `Vec` (Bybit, spot only, still). A venue porting more than
   one of its own native source ids — OKX's `okx-spot`/`okx-swap`/
   `okx-futures`/`okx-option-btc-usd`/`okx-option-eth-usd` — embeds one
   component per id: `wit/senken.wit`'s `venue.venue-descriptor.id` is
   read once per loaded component, so one component cannot answer for more
   than one source id, and the id is the source half of every saved chart
   layout's instrument id, so it must not change when the market moves off
   native. Each extra component is its own tiny crate under
   `plugins/<venue>/wasm-<market>/`, built with
   `plugins/build-venue.sh <venue> wasm-<market> <venue>-venue-<market>`
   (the three-argument form; see that script's own usage docs).
6. **Nothing to change in `crates/runtime`/`apps/cli`** —
   `load_static_venue_components` already loops over every activated
   static plugin's `venue_components()` and registers each; verify, don't
   add a path.
7. Two mutation checks before done: bump a price scale in `<venue>-core`
   (equivalence test fails), diverge one normalisation call site (its unit
   test fails). For a derivative market, two more: drop a dated contract's
   expiry, and mis-scale its option strike (OKX's own
   `options_carry_a_strike_and_a_right` test did not catch the second one
   until its assertion was tightened from `strike > 0` to the exact
   `(strike_scale, strike)` pair — a weak assertion is not a passing test).
   Revert every mutation.
8. `cargo fmt`; `clippy -p <venue> -p <venue>-core --all-targets -- -D
   warnings`; `RUSTDOCFLAGS="-D warnings" cargo doc … --no-deps`;
   `cargo machete plugins/<venue>{,/core,/wasm}`. Trap:
   `-D rustdoc::private-intra-doc-links` fails if a **public** fn's doc
   links `` [`RawNum`] `` (private) — OKX only does that from private fns;
   use plain backticks from a `pub` one.
9. **A derivative market's own wasm crate has no bars unless the native
   adapter already registered a `BarSource` for it.** OKX's swap/futures
   both had one (`bar_source(market, client)`) and kept it, ported to the
   wasm side the same way spot's bars were; OKX's options never had one
   (`activate_with_http` registered a `MarketDataSource` for each option
   family but no `BarSource`), and that component's own `bars` export
   returns `Err(VenueError::Rejected(…))` with `supported_specs()` empty
   and `max_rows()` zero — preserving that exact scope rather than
   guessing whether the venue's history endpoint even accepts an option's
   `instId`.

**Per-venue, not mechanical**: the closure rule, whether the normaliser is
already its own function, markets-per-endpoint, and whether the catalog
paginates (no multi-page fixture → ship single-page, say so in the wasm
crate's docs, don't invent a cursor shape). Bybit took ~1 hour hands-on
against OKX's ~2 days — the fixed SDK/script/harness cost was already
paid. Porting OKX's four derivative markets after spot cost under an hour
each, once `okx-core`'s parser already returned `kind`/`contract`: the
fixed cost there was extending `okx-core` and `wit/senken.wit` once, not
paid again per market.
