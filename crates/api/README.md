# senken-api

The HTTP surface for the single-binary shell: the auth surface (login,
logout, set-password, `me`, and a WebSocket endpoint authenticated through
a short-lived ticket exchange), user/role/grant management, and
workspaces/layouts/panes/pane items, bars, indicators and alerts. Transport
only — authorisation itself lives in
`senken-acl`/`senken-identity`/the guarded stores (`senken-chart`,
`senken-alerts`) themselves; this crate only wires HTTP onto them.

```rust,ignore
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use senken_api::{serve, ServeOptions};
use senken_identity::IdentityStore;
use senken_runtime::Runtime;

let identity = Arc::new(IdentityStore::open("accounts.db")?);
// `runtime` is what `bars`/`indicators` resolve a request against
//  — built once by the caller, e.g. via `senken_cli::runtime_with_plugins`.
let runtime = Arc::new(Runtime::builder().data_dir(".data").build()?);
let handle = serve(
    ServeOptions {
        host: IpAddr::V4(Ipv4Addr::LOCALHOST),
        port: 0,
        allowed_origins: Vec::new(),
    },
    identity,
    runtime,
)
.await?;

println!("listening on {}", handle.local_addr());
handle.shutdown().await?;
```

`serve` and `gui` (in `apps/senken`) both call this one function — that is
what keeps the two modes from diverging.

## Endpoints

| Method | Path | Auth |
|---|---|---|
| `GET` | `/api/health` | public — `needs_setup: bool` reports whether the seeded default admin still has no password, so a login page can decide "set a password" vs. "log in" before any session exists |
| `GET` | `/api/openapi.json` | public — the OpenAPI document `utoipa` derives from this crate's own `serde` structs |
| `POST` | `/api/login` | public (rate-limited per account and per source address) |
| `POST` | `/api/logout` | session |
| `POST` | `/api/set-password` | session, **except** while the target account is fenced |
| `GET` | `/api/me` | session — also reports the caller's role names and effective grants, for cosmetic UI use only |
| `GET` | `/api/me/zone` | session — the caller's own stored display (timezone) zone, or `null` if never chosen |
| `PUT` | `/api/me/zone` | session — sets the caller's own display zone; `IdentityStore::get_zone`/`set_zone` take the target `user_id` straight from the resolved session, never from the request, so there is no way to name a different account here |
| `POST` | `/api/ws/ticket` | session — mints a single-use, ~30s ticket |
| `GET` | `/api/ws` | the ticket from `/api/ws/ticket`, presented in the query string |
| `GET` | `/api/users` | session — scoped by `IdentityStore::list_users` itself |
| `POST` | `/api/users` | session — `IdentityStore::create_user` checks `Create`/`User` itself |
| `GET` | `/api/roles` | session — scoped by `IdentityStore::list_roles` itself |
| `POST` | `/api/roles` | session — `IdentityStore::create_role` checks `Create`/`Role` itself |
| `POST` | `/api/users/{user_id}/roles` | session — `IdentityStore::assign_role` checks `Edit`/`User` itself — assigns an existing role |
| `POST` | `/api/users/{user_id}/grants` | session — `IdentityStore::grant_direct` checks `Edit`/`User` itself — attaches a direct `(Action, Resource, Scope)` grant |
| `POST` | `/api/users/{user_id}/grants/revoke` | session — `IdentityStore::revoke_direct` checks `Edit`/`User` itself |
| `POST` | `/api/users/{user_id}/plugin-grants` | session — `IdentityStore::grant_plugin_permission_to_user` checks `Edit`/`User` itself — grants an opaque, already-registered plugin permission by name |
| `POST` | `/api/users/{user_id}/plugin-grants/revoke` | session — `IdentityStore::revoke_plugin_permission_from_user` checks `Edit`/`User` itself |
| `POST` | `/api/roles/{role_id}/plugin-grants` | session — `IdentityStore::grant_plugin_permission_to_role` checks `Edit`/`Role` itself |
| `POST` | `/api/roles/{role_id}/plugin-grants/revoke` | session — `IdentityStore::revoke_plugin_permission_from_role` checks `Edit`/`Role` itself |

Every route is added through `mount()` (private, in `auth.rs`), whose
`permission: EndpointPermission` argument is required — omitting it is a
compile error (`E0061`), not a review comment. This is how the "every endpoint declares its required permission, checked before dispatch"
is enforced.

The two list endpoints, `create_user`, `create_role`, `assign_role` and
`grant_direct`, and — as of Q10.1 — `revoke_direct` and the
four plugin-grant methods, are all mounted at plain `Authenticated`:
`senken_identity::IdentityStore` performs the same
`AuthenticatedUser::authorize` check on every one of these mutations
**itself**, closing a bypass a non-HTTP caller previously had:
authorisation belongs in the domain crate precisely because a headless
backtest or CLI has no HTTP layer to inherit a
router-level guard from), so an ordinary user hitting a management
mutation gets `403` (not `401`) — a valid session, correctly identified,
just not permitted. `EndpointPermission` no longer has a third, `Acl`
variant: once Q10.1 moved the last of these nine mutations' checks into
`senken-identity`, a second router-level gate in front of any of them
would only ever have checked the same thing twice, never tighter, so the
variant was removed rather than left unused.

### The B4 fence

While an account's password is unset, every endpoint except `set-password`
returns `403`. Binding to a non-loopback address while the default admin is
still fenced logs a `tracing::warn!` on **every request**, not just at
startup.

### Detecting "still authenticated"

`GET /api/health` needs no credential, so a successful poll of it is not
evidence of a live session — polling it as a heartbeat reads as
`authenticated` even after signing out, until the poll interval catches up.
**Poll `GET /api/me` instead**: it requires
`EndpointPermission::Authenticated`, so `200` means a real, unfenced
session and `401` means the credential is gone, immediately rather than on
the next heartbeat tick.

### WebSocket auth

A browser cannot set an `Authorization` header on a WebSocket handshake, so
the client requests a single-use ticket over REST first and presents that —
never the session token — in the `GET /api/ws` query string. See `src/ws.rs`.

### Security

Credentials travel as `Authorization: Bearer`, never a cookie, so there is
no CSRF surface and no CSRF machinery. CORS denies cross-origin requests by
default; `ServeOptions::allowed_origins` is the only way to add one, and
there is no wildcard option. Login is rate-limited per account and per
source address (`identity_handlers::LoginRateLimiter`). Unknown-email and
wrong-password produce the identical response shape and status.

## Workspaces, bars, indicators and alerts

Every route below is mounted at plain `EndpointPermission::Authenticated`:
`senken_chart::ChartWorkspaceStore`/`senken_alerts::AlertStore` perform their
own `AuthenticatedUser::authorize` check on every read and write (the same
pattern Q9.3/Q10.1 established for `senken-identity`), and bars/indicators
have no per-row owner to scope against at all — a valid, unfenced session is
the whole permission story for those two. The three `/api/indicators/plugins*`
routes, plus `/api/indicators/compile`, are the exception in this section:
each handler calls `AuthenticatedUser::authorize` on `Resource::Indicator`
itself, the same shape `/api/storage` uses, since an uploaded or compiled
`.wasm` component runs for every user of this server rather than belonging
to the account that uploaded or compiled it.

`senken-chart` stores a pane's items (computed indicators, referenced
overlay instruments, anchored drawings) in one table now, but the wire API
keeps `layers`/`drawings` as two shapes on purpose — see
`workspace_handlers`' own module docs for why.

| Method | Path | Notes |
|---|---|---|
| `GET` | `/api/workspaces` | scoped by `ChartWorkspaceStore::list_workspaces` itself |
| `POST` | `/api/workspaces` | |
| `GET` | `/api/workspaces/default` | "default-on-first-open belongs on the server" — creates the caller's default workspace and layout on the first call, returns the same pair on every later one |
| `PATCH` | `/api/workspaces/{workspace_id}` | rename |
| `DELETE` | `/api/workspaces/{workspace_id}` | cascades to its layouts/panes/items |
| `GET` | `/api/workspaces/{workspace_id}/layouts` | a workspace's tabs |
| `GET` | `/api/layouts/{layout_id}` | one layout with its full nested pane/item structure |
| `PUT` | `/api/layouts/{layout_id}` | replaces a layout's whole pane/item structure in one transaction |
| `PATCH` | `/api/layers/{layer_id}` | in-place edit of one computed/referenced item, no structural rewrite |
| `DELETE` | `/api/layers/{layer_id}` | |
| `PATCH` | `/api/drawings/{drawing_id}` | in-place edit of one anchored item |
| `DELETE` | `/api/drawings/{drawing_id}` | |
| `GET` | `/api/bars/plan` | pure inspection — `senken_loader::SeriesLoader::plan`, no network, no work started |
| `GET` | `/api/bars/range` | `SeriesLoader::resolve` — cache → store → aggregate from a finer stored spec, never a fetch; what a chart actually renders bars from |
| `POST` | `/api/bars/ensure` | `SeriesLoader::ensure` — enqueues the fetch and returns immediately with a job reference; never blocks |
| `GET` | `/api/bars/jobs/{job_id}` | polls the job `ensure` started |
| `GET` | `/api/indicators` | the catalogue of `senken-indicators`' ten built-ins, plus every currently-enabled indicator loaded from an uploaded `.wasm` component (`senken_runtime::DynamicIndicators`) |
| `POST` | `/api/indicators/compute` | replays already-resolvable bars through the named indicator — a built-in via `senken_alerts::ConcreteIndicator` (reused rather than a second factory), or a dynamic one via `DynamicIndicators::spawn` — one point per bar once `initialized()` |
| `POST` | `/api/indicators/compile` | compiles indicator-lang source (`senken_indicator_lang::compile`) and registers the result the same way an upload does; a mistake in the source comes back as `line`/`column`/`message` verbatim, never folded into the crate-wide `{error}` shape; `Action::Create` on `Resource::Indicator` at `Scope::All` |
| `POST` | `/api/indicators/plugins` | registers a compiled `wasm32-wasip2` component (raw bytes, `Content-Type: application/wasm`) as a dynamic indicator; `Action::Create` on `Resource::Indicator` at `Scope::All`, the same "not owned by any one account" shape `/api/storage` uses |
| `GET` | `/api/indicators/plugins` | every registered dynamic indicator, including one that never finished loading — unlike `/api/indicators`, which only ever lists what a chart may place right now. Each entry carries its own `origin` (`built_in`/`uploaded`/`data_directory`), `state` (`active`/`disabled`/`incompatible`/`failed_to_load`/`auto_disabled`, the last from its own circuit breaker tripping), runtime `health` and ring `logs` — this is the one HTTP surface for diagnosing a broken plugin, not only listing the working ones; `Action::View` at `Scope::All` |
| `POST` | `/api/indicators/plugins/{name}/enabled` | flips whether `/api/indicators` currently offers this dynamic indicator, without discarding the loaded component; `enabled: true` also resets this plugin's circuit breaker if it had tripped (`auto_disabled`) — the breaker never clears itself on its own, since a guest trap's cause is a deterministic bug rather than a transient venue failure (see `senken_plugin_host::circuit`'s own docs); `Action::Edit` at `Scope::All` |
| `GET` | `/api/alerts` | scoped by `AlertStore::list_alerts` itself |
| `POST` | `/api/alerts` | refuses an indicator that cannot even be built before it is ever persisted |
| `GET` | `/api/alerts/{alert_id}` | includes fired-state: `last_fired_at`/`last_fired_value`/`fire_count` |
| `DELETE` | `/api/alerts/{alert_id}` | |

Every bars/indicators key is built with `Origin::Derived`: a chart asks for
a timeframe, not for "whatever the venue itself calls this spec", so the
ladder can always aggregate from a stored finer spec instead of fetching
the requested spec directly. `plan()` and `ensure()` staying separate calls
is load-bearing, not incidental: a client can show "3 months
missing, ~4 minutes" before starting anything. Opening the same range twice
issues zero venue requests the second time — `resolve()`
never fetches, and `ensure()`'s own gap check finds nothing missing once
the first call has written it.

`AlertStore::all_enabled_for_engine`/`record_fire` are deliberately **not**
mounted anywhere in this crate (see that store's own docs) — they answer
"what does the server need to keep running", never a caller's own request.

## Dashboard workspaces and grid

A dashboard workspace is its own aggregate, separate from a chart workspace
— see `senken_dashboard`'s own module docs for why. Every route but the
widget catalog is mounted at plain `EndpointPermission::Authenticated`:
`senken_dashboard::DashboardWorkspaceStore` performs its own
`AuthenticatedUser::authorize` check on every read and write, the same
pattern `senken-chart`'s own workspace routes use.

| Method | Path | Notes |
|---|---|---|
| `GET` | `/api/dashboard/workspaces` | scoped by `DashboardWorkspaceStore::list_workspaces` itself |
| `POST` | `/api/dashboard/workspaces` | |
| `GET` | `/api/dashboard/workspaces/default` | creates the caller's default dashboard workspace on the first call, returns the same one on every later one |
| `PATCH` | `/api/dashboard/workspaces/{workspace_id}` | rename |
| `DELETE` | `/api/dashboard/workspaces/{workspace_id}` | cascades to its widgets; healed by the next `.../default` open, the same way a chart workspace is |
| `GET` | `/api/dashboard/workspaces/{workspace_id}/layout` | the workspace's full widget grid |
| `PUT` | `/api/dashboard/workspaces/{workspace_id}/layout` | replaces the whole grid in one transaction — add, move, resize and delete are all just different snapshots through this one call, guarded by `expected_revision` (`409` on a stale one) |
| `GET` | `/api/dashboard/widgets/catalog` | every widget type this build's server knows how to serve — pure, in-memory data with no owner, so any authenticated caller sees the same catalog |

A placed widget stores a provider id and a widget type id, never a
component — see `senken_dashboard::WidgetRecord`'s own docs for why that is
what makes a placeholder possible when a widget's provider is no longer
available. Every geometry field is grid columns and rows, never pixels.

## Indicator registry

Publish, search, install and revoke indicator-lang source — see
`senken_indicator_registry`'s own module docs for why the registry
publishes source rather than a compiled binary. Publishing, listing one's
own entries, revoking one, and reading/setting one's own handle need a
session (`senken_indicator_registry::RegistryStore` performs its own
guarded check on each); searching, reading and installing a published
indicator need no account at all — a published indicator is public and
installable by design.

| Method | Path | Notes |
|---|---|---|
| `POST` | `/api/registry/indicators` | publishes into the caller's own namespace — the request body carries no `namespace` field, so it can never be steered into another account's |
| `GET` | `/api/registry/indicators` | public search across every namespace; `?query=&limit=&offset=` |
| `GET` | `/api/registry/indicators/mine` | the caller's own published entries |
| `GET` | `/api/registry/indicators/{namespace}/{name}` | public; `{namespace}` accepts a raw account id or a claimed handle (`@alice` or `alice`) |
| `POST` | `/api/registry/indicators/{namespace}/{name}/install` | public; compiles the published source **on the installing machine** and returns the compiled `wasm32-wasip2` component (`Content-Type: application/wasm`), with the language version it compiled against in `X-Indicator-Language-Version` |
| `DELETE` | `/api/registry/indicators/{name}` | revokes the caller's own entry — always the caller's own namespace, never named in the request |
| `PUT` | `/api/registry/handle` | claims (or replaces) the caller's own human-readable registry handle; publishing refuses to run until this has succeeded at least once |
| `GET` | `/api/registry/handle` | the caller's own claimed handle, or `null` |

## Dynamic widget UI packages

A widget UI package is a self-contained bundle (`manifest.json` plus a
`web/` directory of static assets) built entirely outside this
repository — no Rust, no compilation on this host — served into a
sandboxed iframe. See `senken_plugin::widget_package`'s own module docs.
Every route below is `EndpointPermission::Authenticated`, with each handler
calling `AuthenticatedUser::authorize` on `Resource::WidgetPlugin` at
`Scope::All` itself: installing code this server will run (even sandboxed)
is an admin action, distinct from `Resource::Storage` (a different
administrative concern than disk usage) and from `Resource::Indicator` (a
different kind of plugin entirely).

| Method | Path | Notes |
|---|---|---|
| `GET` | `/api/widget-plugins/catalog` | every widget every currently active package contributes — merge with `/api/dashboard/widgets/catalog` for the full effective catalog |
| `GET` | `/api/widget-plugins` | every installed package, enabled or not |
| `POST` | `/api/widget-plugins` | installs a package from the raw bytes of a zip archive (`Content-Type: application/zip`) |
| `POST` | `/api/widget-plugins/{id}/enabled` | flips whether this package's widgets are in the effective catalog, without touching its files or any placed instance's stored config |
| `DELETE` | `/api/widget-plugins/{id}` | removes a package's files entirely — refuses with `400` for the built-in package this server installs on every fresh start (`senken_plugin::widget_package::BUILTIN_PACKAGE_ID`); disable it instead |
| `POST` | `/api/widget-plugins/refresh` | an explicit rescan of the data directory — refresh is always explicit, never a filesystem watcher, since a watcher can fire mid-copy and read a half-written file |
| `GET` | `/widget-plugin-assets/{id}/{*path}` | **public**, and deliberately outside `/api` (meant to eventually move to a genuinely separate origin) — streams one static file out of a package's own `web/` directory into the sandboxed iframe. Answers with plain HTTP status codes, not the crate's usual `{error}` JSON envelope: a missing asset 404s the way any static file server does |

A package dropped by hand under `<data_dir>/widget-plugins/packages/<id>/`
is picked up the moment anything calls `list`/`refresh` — this store
re-reads its directory from disk on every call rather than caching an
in-memory catalog, so there is nothing to explicitly "scan" the way dynamic
indicators are (see the next section).

`senken_runtime::RuntimeBuilder::build` calls
`WidgetPackageStore::ensure_builtin_installed` once at startup, so a fresh
install's `GET /api/widget-plugins` is never simply empty. It is a real,
working package — `examples/widget-plugins/example-clock`, compiled straight
into this binary — not a second, invisible install path: it goes through
`install` the same as an upload would, an admin can disable it like any
other package, and a restart never installs a second copy.

## Dynamic indicators from the data directory

Besides an upload through `POST /api/indicators/plugins` or a compile
through `POST /api/indicators/compile`, a `.wasm` component dropped by hand
under `<data_dir>/indicator-plugins/*.wasm` is registered automatically —
with `origin: data_directory` — when `senken_runtime::Runtime` starts. One
broken file there never aborts startup: it becomes a `failed_to_load` (or
`incompatible`) entry, visible through `GET /api/indicators/plugins`, and
every other file still loads.

## The `web` feature

Serves the SvelteKit build in `web/` (repo root), embedded via `rust-embed`,
with an SPA fallback: any unmatched path other than `/api/*` returns
`index.html` so client-side routing survives a hard refresh.

`rust-embed` reads assets from disk in a debug build and embeds them in
release, so `cargo check`/`cargo test` never need Node or a built `web/`
present — but the `#[derive(RustEmbed)]` folder must still *exist* at
compile time in both modes (verified empirically). `web/build/.gitkeep`
is committed for exactly this
reason: it keeps the directory present on a clean checkout even though its
real contents (`web/build/*`) are gitignored build output.

Without `web`, unmatched non-`/api` paths return a plain 404 — the crate
compiles and runs with zero web assets in the binary
(`cargo check -p senken-api --no-default-features`).
