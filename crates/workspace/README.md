# senken-workspace

Chart workspace persistence for Senken: workspace → layout → panes →
layers/drawings, as SQLite records with an owner rather than JSON blobs.

- **Shares `senken-identity`'s accounts database.** Workspaces reference
  users, so their tables live in the same SQLite file at `.data/accounts/` rather than a second one. `senken-identity` stays the
  file's single owner of `PRAGMA user_version`; this crate never opens its
  own connection to the file, only a clone of that store's connection via
  `IdentityStore::shared_connection`. See the crate's module docs
  (`src/lib.rs`) for the full reasoning.
- **The same guarded-query pattern as `senken-identity`.** Every read and
  write takes an `AuthenticatedUser` and calls `authorize` before touching a
  row; the `Scope` that comes back becomes a `WHERE` clause (or, for a
  single-row operation, a check against that row's owner), including in
  every listing's total row count.
- **A layout change is transactional.** `WorkspaceStore::replace_layout`
  rewrites a layout's whole pane/layer/drawing structure inside one SQLite
  transaction, so a failure partway through — a duplicate grid position,
  say — rolls back to the layout's previous state rather than leaving it
  with fewer panes than it ever actually had.
- **Opening charts with no workspace creates one.**
  `WorkspaceStore::get_or_create_default_workspace` returns an account's
  existing default workspace and layout, or creates both (seeded with one
  workable placeholder pane, never an empty state) the first time — and
  never creates a second one on a later call.
- **Drawings are objects, not paint.** A pane holds zero or more drawings —
  a horizontal line, a trend line, or a rectangle — each with an id, its own
  geometry, and a colour/width/line-style, so a client can hit-test, select,
  edit and delete one individually even though persistence itself still
  goes through the same whole-layout `replace_layout` call as everything
  else here.
- **Out of scope here**: validating that a stored instrument is a real,
  catalogued one, interpreting an indicator layer's parameters beyond
  checking they parse as JSON, strategy layers.
