# senken-api

The HTTP surface for the single-binary shell: the auth surface (login,
logout, set-password, `me`, and a WebSocket endpoint authenticated through
a short-lived ticket exchange), user/role/grant management, and
workspaces/layouts/panes/layers, bars, indicators and alerts. Transport
only — authorisation itself lives in
`senken-acl`/`senken-identity`/the guarded stores (`senken-workspace`,
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
`senken_workspace::WorkspaceStore`/`senken_alerts::AlertStore` perform their
own `AuthenticatedUser::authorize` check on every read and write (the same
pattern Q9.3/Q10.1 established for `senken-identity`), and bars/indicators
have no per-row owner to scope against at all — a valid, unfenced session is
the whole permission story for those two.

| Method | Path | Notes |
|---|---|---|
| `GET` | `/api/workspaces` | scoped by `WorkspaceStore::list_workspaces` itself |
| `POST` | `/api/workspaces` | |
| `GET` | `/api/workspaces/default` | "default-on-first-open belongs on the server" — creates the caller's default workspace and layout on the first call, returns the same pair on every later one |
| `PATCH` | `/api/workspaces/{workspace_id}` | rename |
| `DELETE` | `/api/workspaces/{workspace_id}` | cascades to its layouts/panes/layers |
| `GET` | `/api/workspaces/{workspace_id}/layouts` | a workspace's tabs |
| `GET` | `/api/layouts/{layout_id}` | one layout with its full nested pane/layer structure |
| `PUT` | `/api/layouts/{layout_id}` | replaces a layout's whole pane/layer structure in one transaction |
| `GET` | `/api/bars/plan` | pure inspection — `senken_loader::SeriesLoader::plan`, no network, no work started |
| `GET` | `/api/bars/range` | `SeriesLoader::resolve` — cache → store → aggregate from a finer stored spec, never a fetch; what a chart actually renders bars from |
| `POST` | `/api/bars/ensure` | `SeriesLoader::ensure` — enqueues the fetch and returns immediately with a job reference; never blocks |
| `GET` | `/api/bars/jobs/{job_id}` | polls the job `ensure` started |
| `GET` | `/api/indicators` | the catalogue of `senken-indicators`' ten built-ins |
| `POST` | `/api/indicators/compute` | replays already-resolvable bars through the named indicator (`senken_alerts::ConcreteIndicator`, reused rather than a second factory), one point per bar once `initialized()` |
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
