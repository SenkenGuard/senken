# Contributing to Senken

Senken is a market-data and trading-research platform in Rust. It handles
broker adapter credentials today and is heading toward executing trades, so
the bar here is the one you would want for software that touches other
people's money.

`AGENTS.md` in the repository root is the engineering source of truth — the
type rules, the architecture, and the reasons behind both. Read it before
your first change. This file covers the mechanics of getting that change
merged.

## Getting set up

```bash
bun install
bun run dev
```

That starts the web app on `:5173` and the server on `:4190`. Rust-only
contributors need neither Bun nor a web build: `cargo check --workspace`
must always succeed on its own.

## Commit messages are load-bearing

Release notes are generated from the commit history, so the format is not a
style preference — an unconventional message is simply left out of the
changelog.

```
type(scope): what changed
```

| Type | Appears under |
|---|---|
| `feat` | Features |
| `fix` | Bug fixes |
| `perf`, `refactor`, `style` | Improvements |
| `docs` | Documentation |
| `test` | Tests |
| `build`, `ci` | Build and CI |
| `chore` | Internal |

A breaking change adds `!` after the type, or a `BREAKING CHANGE:` footer.

Scope is the crate or area: `feat(charts):`, `fix(okx):`, `perf(loader):`.

Pull requests are squash-merged, so **the pull request title becomes the
commit message.** Write the title in this form.

## Before you open a pull request

While you are working, test only what you are changing:

```bash
cargo test -p senken-<crate>
cargo clippy -p senken-<crate> --all-targets -- -D warnings
bun run --filter web test <file>
```

Running the whole workspace after every small edit is not thoroughness, it
is a tax — it gets slow enough that it stops being run at all.

Once, before you submit:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace
bun run --filter web check
```

CI runs all of this plus the feature matrix, three operating systems, an
MSRV check and an unused-dependency check. It is a gate, not a report.

## What reviewers look for

- **A test that proves the property**, not one that mirrors the
  implementation. The strongest evidence you can offer is that you removed
  the fix and watched the test fail with the wrong value you predicted.
- **No lint suppressions.** `clippy::pedantic` is on and `unsafe_code` is
  forbidden. There is one documented `#[allow]` in the whole workspace. If
  you reach for a second, fix the code instead — or argue for it in the pull
  request rather than adding it quietly.
- **Money is exact.** Never `f64` for a price, a quantity or an amount;
  those are scaled integers from the venue's wire format through to storage.
  Indicator values may be `f64`, and that boundary is documented in
  `crates/indicators`.
- **Venue facts are recorded, never remembered.** Rate limits, row caps,
  pagination direction and message shapes come from a real response you
  captured, or from a conservative default commented as an assumption.
- **Comments explain why.** A comment earns its place when it records
  something the code cannot say. Never cite a plan or design document by
  number — those files are not in the repository, and a reader cannot follow
  the reference.

## Where things live

`crates/` are independently usable libraries. `plugins/` are venue adapters.
`apps/` assemble them into the binary. `packages/web` is the SvelteKit app
embedded into that binary.

`packages/web/src/lib/api/generated.ts` is generated from the server's own
types. Regenerate it; never hand-edit it.

## Releases

Maintainers cut releases with `scripts/release.sh`. Contributors do not need
to touch versions — every version in the tree is bumped by that script, and
a tag that disagrees with it is rejected by CI before anything is built.
