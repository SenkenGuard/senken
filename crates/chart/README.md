# senken-chart

Chart persistence for Senken: chart workspace → layout → panes → pane items
(indicators, overlaid instruments, and drawing objects, unified), as SQLite
records with an owner rather than JSON blobs.

Named `senken-chart`, not for the workspace concept alone: a dashboard is
its own, separate aggregate with its own workspace concept, so "workspace"
alone stopped naming one aggregate the day a second one existed.

- **Shares `senken-identity`'s accounts database.** Chart workspaces
  reference users, so their tables live in the same SQLite file at
  `.data/accounts/` rather than a second one. `senken-identity` stays the
  file's single owner of `PRAGMA user_version`; this crate never opens its
  own connection to the file, only a clone of that store's connection via
  `IdentityStore::shared_connection`. See the crate's module docs
  (`src/lib.rs`) for the full reasoning.
- **The same guarded-query pattern as `senken-identity`.** Every read and
  write takes an `AuthenticatedUser` and calls `authorize` before touching a
  row; the `Scope` that comes back becomes a `WHERE` clause (or, for a
  single-row operation, a check against that row's owner), including in
  every listing's total row count.
- **A layout change is transactional.** `ChartWorkspaceStore::replace_layout`
  rewrites a layout's whole pane/item structure inside one SQLite
  transaction, so a failure partway through — a duplicate grid position,
  say — rolls back to the layout's previous state rather than leaving it
  with fewer panes than it ever actually had.
- **Opening charts with no workspace creates one.**
  `ChartWorkspaceStore::get_or_create_default_workspace` returns an
  account's existing default workspace and layout, or creates both (seeded
  with one workable placeholder pane, never an empty state) the first time —
  and never creates a second one on a later call.
- **One table for every pane item.** A layer and a drawing were always the
  same concept — a positioned, orderable, show/hideable thing attached to a
  pane — so both are `PaneItemRecord`s now, distinguished by `ItemSource`
  (`Computed` for an indicator, `Referenced` for an overlaid instrument,
  `Anchored` for a drawing) rather than by living in two different tables.
  `visible` applies to all three; placement (main pane vs. sub-pane) is an
  orthogonal `Slot`, not baked into the source's own tag.
- **Drawings are objects, not paint.** A pane holds zero or more anchored
  items — a horizontal line, a trend line, a rectangle, a ray, a Fibonacci
  retracement, or a text note — each with an id, its own geometry, and a
  style, so a client can hit-test, select, edit and delete one individually
  even though persistence itself still goes through the same whole-layout
  `replace_layout` call as everything else here.
- **Out of scope here**: validating that a stored instrument is a real,
  catalogued one, interpreting an indicator's parameters beyond checking
  they parse as JSON, strategy layers, rendering (`senken-indicators` owns
  the `Drawable`/`ToolDescriptor` output contract these anchored items are
  eventually drawn through).
