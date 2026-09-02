# senken-indicator-registry

Publishes, searches, and installs indicator-lang source — never a compiled
binary.

## Source, never a binary

`senken_indicator_lang::compile` runs on the *installing* machine, every
time. Publishing a compiled artifact instead would recreate a problem this
design does not have to solve: nothing could prove a published binary
actually came from the source sitting next to it, short of recompiling it
anyway. Publishing source means what you read is what you run, with no
build farm to operate or secure, and every published artifact is a few
kilobytes of readable, forkable, reviewable text.

## What actually needs defending: identity and naming

With binary provenance off the table, what is left is publisher identity
and naming — the class of attack where a malicious publisher impersonates
or typosquats a legitimate one to get their code installed under a
trusted-looking name.

- **Every name is namespaced by its publishing account.** A qualified name
  is `{namespace}/{name}`, where `namespace` is the publishing account's
  own id — never a self-chosen display string a impersonator could also
  choose. `(namespace, name)` is the stored uniqueness, so two authors may
  use the same `name` in their own namespaces without colliding, and
  [`RegistryStore::publish`] refuses a `namespace` argument that is not the
  caller's own account before anything else runs.
- **A `Handle` is what a human actually types.** Nobody types
  `@550e8400-e29b-41d4-a716-446655440000/supertrend`. A handle is a
  validated (lowercase letters, digits, hyphens only), globally unique
  name an account claims once and that resolves back to its `UserId` — it
  never replaces the account id as the stored namespace, only gives it a
  human-facing address. Uniqueness is enforced the same way
  `(namespace, name)` already is, by a database constraint, not a
  convention, so it closes the same impersonation question a
  handle-only design would otherwise reopen. `RegistryStore::publish`
  refuses to run for an account with no handle chosen yet — a published
  entry nobody can address is not meaningfully published.
- **The indicator language's version is recorded on every publish.** An
  installing host too old for what it fetches is refused with a message
  naming both versions, never a silent failure to load.

## Revoking a publish

`RegistryStore::delete` lets an author remove their own published entry —
and only their own: it reuses `publish`'s own namespace-ownership check,
so this is never something a wider grant can extend to someone else's
entry. Deleting removes the entry from search and blocks any *new*
install; it cannot reach into a copy someone already installed, since
installing copies the compiled bytes to the installing machine rather
than leaving a live reference back to this registry.

## Publishing needs an account; installing does not

`publish`, `delete` and `list_mine` take an `AuthenticatedUser` and go
through `AuthenticatedUser::authorize`, the same guarded-query shape
`senken-notes`/`senken-dashboard` use. `search`, `get` and `install` take no
account at all — a published indicator is public and installable by
design, the same way this workspace already treats market data as global.
`set_handle`/`get_handle` take a bare `UserId`, not an `AuthenticatedUser`
— choosing your own address needs no grant, matching
`senken-identity`'s own `set_zone`.

## What is deliberately out of scope

Signing and a trust root, moderation, and ratings/reviews are not this
crate's job. Nor is a version history per indicator: publishing again under
a name an account already owns replaces that entry's source in place.

## One database, one schema-version owner

Registry entries and registry handles both reference `users(id)`, so
their tables live in the same SQLite file `senken-identity` already owns
rather than a second database this crate would have to keep referentially
consistent with the first by hand. `senken-identity` stays the file's
single owner of `PRAGMA user_version`; this crate never opens its own
connection, only a clone of that store's connection via
`RegistryStore::new`.
