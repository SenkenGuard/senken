# senken-watchlist

Watchlist persistence for Senken: a user-owned group of watched instruments,
and its membership, as SQLite records rather than a JSON blob.

- **Shares `senken-identity`'s accounts database.** Watchlist groups
  reference users, so their tables live in the same SQLite file at
  `.data/accounts/` rather than a second one. `senken-identity` stays the
  file's single owner of `PRAGMA user_version`; this crate never opens its
  own connection to the file, only a clone of that store's connection via
  `IdentityStore::shared_connection`.
- **The same guarded-query pattern as `senken-identity`/`senken-chart`.**
  Every read and write takes an `AuthenticatedUser` and calls `authorize`
  before touching a row; the `Scope` that comes back becomes a `WHERE`
  clause (or, for a single-row operation, a check against that row's
  owner), including in every listing's total row count.
- **A member's owner is read through its group.** `watchlist_members` has
  no `owner_id` column of its own — the same relationship `senken-chart`'s
  panes have to their workspace — so deleting a group cascades its members.
- **Adding a duplicate instrument is idempotent.** `add_member` returns the
  existing row rather than erroring when a group already holds the
  instrument.
- **Instruments are typed.** A group's members are stored and returned as
  `senken_marketdata::InstrumentId`, never a bare `String`; a row that no
  longer parses is a distinct, named error rather than a silently skipped
  one.
- **Out of scope here**: validating that a stored instrument is a real,
  catalogued one, anything about how a watchlist is rendered.
