# senken-notes

Note persistence for Senken: a user-owned freeform note (title and body) as
a SQLite record rather than a JSON blob.

- **Shares `senken-identity`'s accounts database.** Notes reference users,
  so their table lives in the same SQLite file at `.data/accounts/` rather
  than a second one. `senken-identity` stays the file's single owner of
  `PRAGMA user_version`; this crate never opens its own connection to the
  file, only a clone of that store's connection via
  `IdentityStore::shared_connection`.
- **The same guarded-query pattern as `senken-identity`/`senken-chart`.**
  Every read and write takes an `AuthenticatedUser` and calls `authorize`
  before touching a row; the `Scope` that comes back becomes a `WHERE`
  clause (or, for a single-row operation, a check against that row's
  owner), including in every listing's total row count.
- **Listings stay small.** `list_notes` returns a note's id, owner, title
  and `updated_at` only — never the body — so a listing page's payload does
  not grow with how much a user has written. `get_note` returns the body.
