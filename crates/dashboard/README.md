# senken-dashboard

Dashboard persistence for Senken: a user-owned workspace and the widgets
placed on its grid, as SQLite records sharing `senken-identity`'s accounts
database rather than a second one.

- **Shares `senken-identity`'s accounts database.** Dashboard workspaces
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
- **A dashboard is its own aggregate, not a second chart workspace.** A
  chart workspace's shape is dictated by its layout preset; a dashboard
  workspace holds arbitrary widgets a user places, moves and resizes
  freely. `senken_acl::Resource::DashboardWorkspace` is its own
  authorisation resource for the same reason.
- **A placed widget stores a provider id and a widget type id, never a
  component.** This is what makes a placeholder possible: when a widget's
  provider is not in a caller's effective `WidgetRegistry` — disabled,
  failed to load, or simply not installed in this build — the widget's
  cell, size and `config` are all still there, untouched, ready for enable
  to bring it back exactly as it was. There is deliberately no foreign key
  from a widget's stored type id to any registry: a registry entry can
  disappear; a stored layout must still read back.
- **Geometry is grid columns and rows, never pixels.** A pixel value bakes
  in whatever screen size happened to be open at save time; a layout saved
  at one window size must read back correctly at another.
- **`replace_layout` writes a workspace's whole widget grid in one
  transaction**, guarded by optimistic concurrency (`revision`): two tabs
  open on the same workspace cannot silently overwrite each other, and a
  failure partway through never leaves fewer widgets than either the old
  or the new state ever had. Grid bounds and pairwise rectangle overlaps
  are validated in Rust before the transaction ever opens — an ordinary
  SQLite constraint cannot reject two overlapping rectangles.
- **`WidgetRegistry`** is a separate, non-persisted catalog of the widget
  types this build's server currently knows how to serve. Deciding
  whether a stored widget renders for real or as a placeholder is a
  caller's job (cross-referencing a stored `widget_type_id` against the
  registry's `contains`/`get`), never this crate's own.
