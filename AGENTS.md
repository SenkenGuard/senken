# Agent Instructions

Shared guidance for coding agents working in the Senken repository. This file is
the repository-level source of truth. Tool-specific instruction files may add to
it but must not contradict it.

## Start with current evidence

Senken changes quickly, and several documents in it are older than the code.
Before describing behaviour or editing a subsystem:

1. Run `git status --short --branch` and preserve every unrelated change.
2. Read the current implementation and its tests.
3. Treat `refs/` as **reference material and plans, not proof that a feature
   exists**. `refs/` is gitignored: it holds the design records, the numbered
   plans, and two vendored competitor repositories kept for study.
4. When a plan and the code disagree, the code is what runs — say so rather than
   choosing silently.

A plan's own line numbers and measurements can be wrong. They have been: one
plan mislabelled a block of the design reference, and an agent that read the
file instead of trusting the map was right to.

---

## What Senken is

A modular market-data and trading-research platform in Rust, built on one rule:
**every domain crate stands on its own, and the runtime is what makes them one
application.**

One binary serves four modes — CLI, `serve` (HTTP + embedded web app), `gui`
(the same server with a desktop window), and a bare invocation that picks
between help and GUI by whether a TTY is present.

**Senken executes trades, and it holds broker adapter credentials. Hold every
change to a standard appropriate to software that touches other people's
money.**

---

## Non-negotiable engineering rules

### Money is exact; indicator values are not

**Never `f64` for a price, a quantity, or money.** These are scaled integers —
`(scale, value)` pairs — from the venue's wire format through to storage. A
venue reporting `1e-06`, `0.00000100` and `1E-6` must land on the same integer.

The one exception, and its boundary is hard: **indicator values may be `f64`**
(an EMA, an RSI, a standard deviation are fractional by nature and are display
and decision values, not money). But **an order price derived from an indicator
must be rounded back to the instrument's tick as a scaled integer** before it
reaches anything that trades. `crates/indicators` allows
`clippy::cast_precision_loss` for itself alone, and documents where `f64` stops
being exact — 2^53. No other crate may.

### One time type, one unit, one zone

`senken_core::UnixNanos` — nanoseconds since the Unix epoch, UTC, everywhere.
Conversions are **checked** and must name their unit (`from_millis`,
`from_secs`); there is deliberately no `From<i64>`.

This exists because it was paid for: Gate reports contract expiry in **seconds**
while every neighbouring field is milliseconds, and that shipped and had to be
found by hand. A raw integer cannot be rejected at a call site. A newtype can.

### Zero lint suppressions

`clippy::pedantic` is on, `unsafe_code` is `forbid`, `missing_docs` warns. There
is not one `#[allow]` in the workspace outside the single documented indicator
exception above. If you reach for one, fix the code instead. If you believe an
exception is genuinely correct, scope it to one crate, argue it in a comment, and
say so in your report — do not add it quietly.

### Make the mistake unrepresentable

Where this project has found a class of bug, it has closed it with a type rather
than a convention. Follow the same instinct, and do not weaken these:

- `SourceSymbol` — `BarSource::bars` takes the venue-native symbol. Passing the
  normalised one is a **compile error**, because it would silently succeed on
  venues where the two forms coincide.
- `senken_acl::Decision` wraps a private enum with no public constructor, so a
  `Scope` cannot exist without an authorisation check having run.
- `senken_identity::AuthenticatedUser` likewise: only `resolve_session` produces
  one, having actually checked a token.
- `Resource` is a closed enum matched exhaustively, so **adding a resource fails
  to compile until its authorisation is written**.
- `senken_subscription::Lease` has no `unsubscribe` method, only `Drop` — a
  leaked subscription is not expressible.
- `senken_trade::SecretString` serialises as `null`, always: there is no path
  through `Serialize` that writes a credential, so an API response or a log
  line cannot leak one even by mistake. Persisting one goes through a
  separate, explicitly named function the account store is the only caller
  of.
- `senken_trade::OrderKind` carries each kind's prices *inside* its variant,
  so a limit order with no limit price — or a market order carrying one —
  cannot be constructed at all.
- Access is resolved **per account**, not per adapter. `AdapterCapabilities`
  stays the most an adapter can ever do; `AccountAccess` narrows that to one
  account, and `TradeEngine` validates every mutation against the account's
  *resolved* capabilities, never the adapter's own maximum. This is the
  correction the MetaTrader 5 investor login forced: one adapter, one set of
  capabilities, two logins with different rights — an order kind the adapter
  supports in general but this account may not is refused before the adapter
  is asked, not after. A new adapter with any account-level restriction
  narrower than "everything the adapter can do" needs this, not a capability
  flag on the adapter itself.

### Derive from what you hold, not from what you opened with

An effect that re-runs must recompute from the **current** state, never from
the value the screen opened with. Where state accumulates — a loaded window, a
list a user has added to, a cache another pass fills in — asking again for the
opening value silently discards everything since.

This is the single most expensive bug class this project has produced, and it
recurs because each instance looks local and harmless:

- A chart's bar effect re-ran on every closed bar and re-requested its
  *opening* 300-bar window, discarding every page of history the reader had
  scrolled in. The window snapped back, which dragged the viewport, which put
  the left edge back in prefetch range, which loaded the same page again — the
  chart oscillated for as long as bars kept closing. It took six wrong fixes
  and a live trace to find, because every individual operation was correct.
- The same shape, in the same file, had the indicator range pinned to the
  opening window while the bars grew: candles extended back and the plots
  stopped dead.
- And again: a frame computed from a cached bar count rather than the bars
  actually being drawn, where the count is a divisor — a stale handful left one
  candle filling the pane.

Two symptoms name it. **State that shrinks back after growing**, and **a
derived value that disagrees with the state it is derived from.** When either
appears, look for the opening value being asked for a second time.

### A venue's symbol is normalised once, by one rule

A plugin normalises a venue symbol in two places — when it builds the
instrument catalog, and again when it decodes a live frame — and the two
must strip exactly the same separators. They are easy to let drift, and the
failure is silent: the decoder builds `BTC_USDT` where the catalog holds
`BTCUSDT`, so every frame is attributed to an instrument nobody leased and
dropped with no error. Deribit and Crypto.com both shipped that way and
were found only by comparing the two call sites across all 22 plugins.

When a venue writes more than one market shape (Deribit's `BTC-PERPETUAL`
beside `BTC_USDT`, Crypto.com's `BTCUSD-PERP` beside `BTC_USDT`), the
separator set must cover every shape the source serves — not just the one
the capture happened to use.

### Fixtures are recorded, never hand-written

Every venue adapter's test data comes from a real response. This is not
tidiness: BitMart's `-1` precision, Phemex's missing spot `tickSize`, Gate's
`order_size_min` of zero on majors, WhiteBIT's silently camelCased struct, and
Deribit's premium-vs-counter currency were each found **only** because the
fixture was genuine.

### Do not invent venue facts

Rate limits, stream caps, row caps, message shapes, pagination direction. None
of these may be written from memory of a venue's documentation. Either confirm
it live and cite it, or use a conservative default **commented as an
assumption**. Documentation is stale; the venue's own response headers are
authoritative.

Two verified examples of why: Binance silently caps `limit` at 1000 on spot
klines and returns HTTP 200 — an implementation trusting the docs loses data
with no error. OKX's `after=X` returns candles **older** than X.

---

## Architecture and code map

Dependencies point downward only: a plugin depends on a domain crate, never the
reverse; the runtime depends on everything and nothing depends on it.

| Crate | Role |
|---|---|
| `core` | `UnixNanos`, scaled integers, path encoding, `TimeRange` — zero I/O |
| `storage` | atomic, versioned JSON snapshots |
| `marketdata` | instruments, cached search, and the `MarketDataSource` and `BookSource` contracts |
| `series` | `Bar`, `BarSpec`, `Origin`, aggregation, `Clock` — pure computation |
| `store` | Parquet series storage; coverage derived from filenames |
| `loader` | resolution ladder, caches, the job model |
| `subscription` | lease pool with `Drop` guards, reference counted; the live-feed ports (`VenueProtocol`, `SymbolMap`, `FeedSource`) |
| `feed` | the WebSocket dial/reconnect engine — generic across venues, no venue in it |
| `indicators` | ten incremental indicators |
| `alerts` | standalone alert evaluation |
| `trade` | the `TradeAdapter` contract, orders/positions/balances, adapter capabilities, dynamic settings schemas, attached accounts |
| `venue` | HTTP, retry, rate limiting |
| `plugin` | the plugin contract |
| `acl` | `Action`, `Resource`, `Scope`, `decide` — no I/O |
| `identity` | users, roles, sessions (SQLite) |
| `chart` | workspaces, layouts, panes, layers |
| `watchlist` | watchlist groups and their instruments |
| `notes` | notes, stored globally and shown per workspace |
| `api` | HTTP surface — transport only |
| `runtime` | composition and lifecycle; owns what the plugins registered |

**Arrow and Parquet appear in `senken-store` and nowhere else**, behind a
feature. Someone who wants bar types, or only wants to inspect what data exists,
never compiles a columnar dependency.

### A port lives in the lowest crate that can express it

Four capabilities are registered through `senken_plugin::ActivationContext`:
`MarketDataSource` (instruments), `BarSource` (bars), `BookSource` (depth) and
`FeedSource` (a live price/quote stream). A venue plugin declares what it can
do by registering; **registration is the capability declaration**, and whatever
reports capabilities to a client reads that absence rather than guessing.

Where each port lives is decided by what it actually needs, and getting this
wrong has already cost this project a whole feature. `VenueProtocol` and
`SymbolMap` sat in `senken-feed` — a crate for *implementations* — which meant
`senken-plugin` could not name them, which meant live streaming could not be a
plugin capability at all, which meant it was hardcoded in `senken-api` for one
venue while twenty-two plugins existed. `BookSource` was one crate too high for
the same reason: it needs only a `SourceSymbol` and a `SourceError`, so leaving
it in `senken-subscription` would have made every venue plugin compile a
subscription pool, a tokio runtime and the indicator crate just to say it can
serve depth.

`FeedSource` is deliberately a **factory**, unlike its three siblings. A
`VenueProtocol` needs a `SymbolMap` built from the instrument catalog, and that
catalog does not exist during activation — assembling it is what activation is
for. So a plugin hands over the means to build a protocol and the runtime calls
it once it holds a catalog. The runtime decides when live data starts; the
plugin only says that it can.

### Where a rule belongs

**The lowest layer that can express it completely.** The test: if the GUI were
deleted, would this rule still be needed? If yes, it is a library concern.

Authorisation lives in the domain crate, not the API, because a headless
backtest run as a user needs the same check and has no HTTP layer to inherit it
from. Auth *transport* — sessions, tokens, login — is the API's.

### Market data is global

Instruments and bars are never tenanted per user. Only user-authored artifacts —
workspaces, layouts, drawings, alerts, strategies, trading accounts — have
owners.

### The adapter owns the money; the engine owns the attachment

A broker or exchange already holds the authoritative orders, positions and
balances for an account. Senken stores none of them — a second copy could
only ever be one that disagrees, at the moment it mattered. What Senken owns
is the *attachment*: which adapter, whose account, under what label, with
which settings. That is `senken-trade`'s `TradeAccountStore`, and it is where
authorisation lives.

Two rules there are stricter than an ordinary scope-limited grant, because
that store holds credentials and decides who can spend:

- **A credential is only ever loaded for the account's own owner.**
  `Scope::All` widens what an operator can *see* about an account, never what
  they can read inside it.
- **Trading is owner-only, whatever the role says.** Administering the
  platform and spending other people's money are different authorities, and
  no grant merges them.

---

## Security rules that are easy to get wrong

- **Hiding UI is not access control.** A menu may be hidden so nobody faces
  controls they cannot use; enforcement is server-side, on every endpoint,
  always. Settings screens are where this is most often forgotten.
- **Scope reaches the `WHERE` clause**, including the total count. Filtering
  after fetching leaks through pagination: the rows are hidden but "1–20 of 340"
  still reports how many exist.
- **A `403` must never log anyone out.** It means authenticated-but-not-permitted
  and shows a message; a `401` is what clears the credential.
- **Credentials never travel in a URL.** A token in a query string outlives the
  session in access logs, proxy logs and browser history. The WebSocket uses a
  single-use ticket for exactly this reason.
- **Broker adapter credentials are per user and never shared**, whatever the
  role.

---

## Verification

**Every feature ships with its tests.** A feature whose behaviour nothing
exercises is not finished, however well it compiles — and the test is written
against the property the feature is supposed to have, not against whatever the
implementation happens to do.

**While iterating, test only the crate or package you are changing.**

```bash
cargo test -p senken-<crate>
cargo clippy -p senken-<crate> --all-targets -- -D warnings
bun run --filter web test <file>
```

Running the whole workspace after every small addition is not thoroughness, it
is a tax: it is slow enough that it stops being run at all, and a check nobody
runs catches nothing. Widen the scope only when your change crosses a crate
boundary.

**Run the full sweep once, at the end, before reporting work done** — that is
where workspace-wide breakage is meant to be caught, and it is not optional:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --no-deps
cargo machete
bun run --filter web check
bun run build:web
```

Feature-matrix checks matter here and have caught real breakage:

```bash
cargo check -p senken-api --no-default-features --all-targets
cargo check -p senken-store --no-default-features --all-targets
cargo check --workspace          # must pass with no Bun and no web build output
```

### Prove the property; do not assert it

A test that passes proves nothing about whether it *would* fail. This project's
best work has consistently done the extra step:

- Add a `Resource` variant, watch the exhaustive match fail to compile, revert.
- Remove a guard, watch the test fail with the predicted wrong value, restore.
- Feed bars in reverse order and confirm 9 of 10 indicators break — and that the
  one that does not is the commutative one.
- Check a computed colour with `getComputedStyle`, not by eye.

**Do not weaken, delete, or rewrite a test to obtain a passing result.** Fix the
underlying problem. Change a test only when the task intentionally changes the
required behaviour, or when you can independently show the test itself is wrong.

### A test must create the state it is about, not wait for it

A test that hopes a race falls its way passes on the machine it was written on
and fails on the machine that runs it. That is worse than a flake: it reports a
bug in code that is correct, and invites the next reader to "fix" a sound
implementation.

The reconnect test proving a lease released while disconnected is not replayed
released it and hoped the redial had not happened yet. A redial takes no
backoff before its first attempt, so on a faster machine the socket came back
first — and replaying an instrument the pool still leased is the *correct*
answer. It went red on Windows CI having proved nothing about the property it
names.

Establish the precondition; do not sleep towards it. Hold the fake venue's
handshake until the release has run, gate on a channel, await the flush that
makes the state true. A negative assertion — "nothing arrives" — is only worth
the window it is given *once the state it is about actually exists*, and until
then it is measuring the scheduler.

### Fix the class, then sweep for the rest of it

Finding one instance is the beginning of the work, not the end. Before moving
on, search for every other place the same shape occurs — the same helper, the
same cached value, the same "recompute from the opening input" — and say in
your report what the sweep covered.

The rule is paid for: the opening-window bug above was found and fixed once for
indicators, and the bars loader beside it had the identical fault. It was not
looked for, and it cost five more rounds of the owner's time. One `grep` for
the helper's other callers would have ended it that day.

### Classes that silently do nothing

Five have shipped in this repository, and each looked exactly like a design
choice: `data-active:` selectors that bits-ui never emits; `bg-pop`, never
exported through `@theme`; `border-ink/18`, a border *colour* with no *width*;
`sm:max-w-md`, clamping every dialog; and a `dark:`-scoped rule that an
unscoped caller cannot override, because tailwind-merge cannot dedupe across
modifier chains.

**Verify any visible state in the DOM.** Tailwind reports nothing for a class
that does not exist.

### Measure where it actually runs

A measurement taken in an environment that does not behave like the real one is
worse than no measurement: it produces confident, wrong conclusions that then
direct the next several hours.

Two readings in this project were pure artefacts of the harness, and both were
believed and acted on: a chart's viewport read as "unchanged" because the
browser pane was hidden and the library's `requestAnimationFrame` loop never
ran, and a race that reproduced only when a fetch resolved in the same tick,
which no real fetch ever does.

Before trusting an observation, confirm the environment does the thing being
observed — force a paint, use the real timings, check the value moves at all
when it obviously should. And when a UI bug resists code reading, an opt-in
trace at every write site, run by the owner on a session that actually fails,
settles in one round what inspection has already failed at several times.

### Generated artifacts

`packages/web/src/lib/api/generated.ts` is produced by `openapi-typescript` from
the server's own `serde` structs. **Regenerate it; never hand-edit it.** It has
already gone stale once and been caught. The same holds for anything else with a
generator: change the source, run the generator.

---

## This machine

- **Do not run two Rust builds concurrently.** It has twice exhausted disk and
  CPU here, killing both.
- `target/` reached 61 GB before `[profile.dev] debug = "line-tables-only"`
  landed. Do not remove that setting to get a stepping debugger without saying
  so.
- **Never run `cargo clean` while another agent is building.** On `ENOSPC`, stop
  and report rather than deleting anything.
- `rm -rf .data` forces a refetch of 50 venue catalogs. Do not do it casually.
- **Binance has banned this machine's IP once (HTTP 418).** Requests to a banned
  endpoint extend the ban. Prefer OKX or Bybit for live checks, and never poll.
- **The trade engine's own UI polls, on a five-second timer.** Refreshing one
  watched account costs four calls (balances, positions, orders, fills) —
  harmless against the local simulator, real requests against a real venue.
  `lib/trade/watch-scope.ts` decides what is actually watched at any moment
  (the whole account list on the Overview, just the active account
  elsewhere), which keeps the cost proportional to what a screen shows
  rather than to every attached account — but does not reduce the
  four-calls-per-account cost itself. There is no adapter-declared refresh
  cadence yet; see `crates/trade/README.md`.

---

## Working rules

- Read the affected code and search for an existing pattern before adding a new
  one. This codebase has established patterns for guarded queries, capability
  objects, and incremental computation — reuse them rather than inventing a
  second shape.
- **Keep each change focused. Note unrelated issues instead of fixing them.**
- **Report a scope decision rather than filling the gap silently.** Two
  responsibilities have fallen between briefs in this project because each side
  reasonably assumed the other owned them — and both were found only because
  both sides said what they had decided.
- Do not add test-only behaviour, branches, or interfaces to production code.
- Expose the minimum public API. Avoid drive-by refactors and renames.
- Say plainly what you could not verify. "I could not exercise this" is a useful
  report; a claim of success you did not observe is not.

## Comments, and what must never appear in shipped code

**Never cite a plan, milestone, or design-record section in code or in the user
interface.** Not `(plan 004 B13)`, not `per D22`, not `milestone R5`. Those
documents live in `refs/`, which is gitignored — a reader of the repository
cannot follow the reference, and a *user* should never see one at all.

This has already shipped: the settings screen once told people their sessions
were signed out "(plan 004 B13)". Anything user-facing must read as product
copy, written for the person using it.

If a decision genuinely needs explaining, **state the reason itself**, briefly,
without the citation:

```rust
// Gate reports expiry in seconds while its neighbours use milliseconds.
let expiry = UnixNanos::from_secs(raw)?;
```

not

```rust
// Per plan 002 M2.2's Gate note, expiry is seconds (see B14).
```

The first survives the plan being deleted. The second does not.

**Comment sparingly.** A comment earns its place when it records something the
code cannot say: a venue quirk, a non-obvious ordering constraint, a rejected
alternative, a limit that will bite later. Narrating what the next line does, or
restating a type, is noise — and a wall of it makes the genuinely important
comment invisible.

Public items need doc comments; that is what `missing_docs` enforces. Everything
else should justify itself.

---

## Git and public interaction

- **The repository has no commits yet, and committing is deferred by the owner.**
  Do not commit, stage, push, branch, or tag unless explicitly asked.
- Do not open, edit, comment on, or review GitHub issues or pull requests unless
  explicitly asked.
- Do not add `Co-authored-by:` trailers for AI tools or models, and do not add
  branded footers such as "Generated with …" to commit messages or PR text.
- If AI assistance is disclosed at all, keep it general and vendor-neutral.

## Repository conventions

- `crates/` are independently usable libraries; `plugins/` and `apps/` assemble
  them.
- `packages/web` is the SvelteKit SPA, embedded into the binary by `rust-embed`.
  Bun is the package manager; one lockfile at the repository root.
- `refs/` is gitignored: design records, numbered plans, vendored competitors.
- Comments explain **why**, not what — and only where the reason is not obvious
  from the code. Test names read as sentences.
- Licence is `LGPL-3.0-only`.
