# senken-indicator-registry

Two stores, kept in one crate because both are "indicators a database row
identifies by owner and name":

- **`UserIndicatorStore`** — the guarded store behind an account's own
  compiled Rust indicators. This is the active one:
  `user_indicator_handlers` in `senken-api` mounts it, and it is what
  backs `POST /api/my/indicators` and the rest of that surface.
- **`RegistryStore`** — a public registry for publishing, searching and
  installing indicator source across accounts. **Not mounted by anything
  today.** It stays here, compiled and tested, for whenever a registry is
  designed again; see its own module docs (`src/store.rs`) for why.

## `UserIndicatorStore`

Follows the same guarded-query shape `senken-notes` uses: every method
takes an `AuthenticatedUser` and goes through `AuthenticatedUser::authorize`
first. Two exceptions treat an indicator's source like
`senken-trade::TradeAccountStore` treats broker credentials — readable and
writable by their owner alone, regardless of a wider role's scope — because
a caller who is not the owner should not learn a given indicator even
exists:

- `get` (reads source and the compiled artifact) and `update_source`
  (writes source) are owner-only, whatever the caller's grants say.
- A caller who is not the owner is told the indicator does not exist,
  never that they may not see it — the same reasoning
  `senken_trade::TradeAccountStore::settings_for` documents for itself.

`list` and `delete` follow the ordinary scope rule instead: `Scope::All`
may see every account's summaries, or delete any account's row — neither
leaks anything reading source would.

A failed compile (`record_compile` with `CompileOutcome::Failure`) leaves
the previously-compiled `wasm`/`api_version` exactly as they were: an
indicator already placed on a chart keeps working after a typo in a later
edit.

## `RegistryStore` (unmounted)

Publishes, searches, and installs indicator source — never a compiled
binary. `publish`'s own handle gate refuses any account with no row in
`registry_handles`, and nothing in this build can put a row there any
more (the only writer, `set_handle`, was removed along with the rest of
the account-handle feature), so every publish attempt is refused today.
`install` no longer compiles the fetched source either — this crate has no
compiler of its own since `senken-indicator-lang` was removed. Both
methods are kept working and tested against everything *except* that gap,
so a future redesign has a store to build on rather than a blank page.

## One database, one schema-version owner

Every table both stores use references `users(id)`, so they live in the
same SQLite file `senken-identity` already owns rather than a second
database this crate would have to keep referentially consistent with the
first by hand. `senken-identity` stays the file's single owner of `PRAGMA
user_version`; this crate never opens its own connection, only a clone of
that store's connection via `RegistryStore::new`/`UserIndicatorStore::new`.
