# senken-identity

The identity store for Senken: users, roles,
grants and sessions in SQLite at `.data/accounts/`, Argon2id
password hashing with parameters fixed by the plan rather than left to a
library default, and a guarded query API that makes an unscoped read
impossible to express.

- **The default superadmin is real but fenced.** `admin@mail.com` is
  created on first run with no password; `users.password_hash` being `NULL`
  *is* that fence — every operation except setting the password refuses to
  run while it is unset.
- **No `list_users()`.** Reading more than one row back requires an
  `AuthenticatedUser`, obtained only by resolving a real session, and the
  `senken_acl::Scope` that comes back from the permission check becomes a
  `WHERE` clause — including in the total row count, so pagination cannot
  leak how many accounts exist beyond what the caller is allowed to see.
- **Sessions are opaque, hashed, and constant-time compared.** A session
  token is 256 bits from the OS RNG; only its SHA-256 digest is ever
  written to `sessions.token_hash`, and looking one up never runs a plain
  `==` over token-derived bytes.
- **Login cannot enumerate accounts.** An unknown email, an account with no
  password set, and a wrong password all return the same error after
  paying the same Argon2 cost.
- **`is_fenced`, `set_password_for`, `get_own_profile`** (added for plan
  004 Q4, the HTTP layer) round out the API a self-service HTTP endpoint
  needs without weakening the guarantees above: `is_fenced` lets an
  unauthenticated `set-password` call be refused once an account is no
  longer fenced (otherwise `set_password` — the operation that *clears*
  the fence — would happily overwrite an already-set password for anyone
  who merely knows an email); `set_password_for` and `get_own_profile`
  let an authenticated caller change or read their own account by
  `UserId`, with no `senken_acl` grant required, the same reasoning
  `set_password` itself documents for why changing your own password
  needs none.
- **`list_roles`, `revoke_direct`, and the plugin-permission grant/revoke
  methods** (closing the gap Q6 found: the HTTP layer had
  nowhere to call `create_user`/`create_role`/`assign_role`/`grant_direct`
  from). `list_roles` is guarded exactly like `list_users`; a role
  has no owning-user column, so `Scope::Own` is read as "the roles this
  actor currently holds" (a join through `user_roles`) rather than "roles
  this actor created." `revoke_direct` is `grant_direct`'s inverse, and the
  four `grant_plugin_permission_to_{user,role}` /
  `revoke_plugin_permission_from_{user,role}` methods attach or remove an
  opaque, already-registered `senken_acl::PluginPermissionName` — granting fails with `PluginPermissionNotFound`/`PluginPermissionOrphaned`
  for a name no plugin has (or no longer has) registered, since a plugin
  may register a permission but this crate never lets anyone grant one
  that does not, or no longer, exist. Every one of these six methods
  invalidates the affected account's (or, for a role change, every
  member's) sessions — a privilege change takes effect on the next
  request, not at the next token expiry, because there is no token to wait
  out.
- **`AuthenticatedUser::authorize` is `pub`, and it gained `role_names()`/
  `effective_grants()`**: `GET /api/me` reports `role_names`/
  `effective_grants` for cosmetic UI use only — B8 still holds, and every
  guarded query re-checks a real grant regardless of what a client was
  told.
- **Every user/role/grant mutation now takes an `AuthenticatedUser`**
  — `create_user`, `create_role`, `assign_role`, `grant_direct`,
  `revoke_direct` and the four plugin-grant methods: each calls
  `authorize` on the same `(Action, Resource)` pair `senken-api` already
  declared for its endpoint before doing anything else, so a caller with
  no HTTP layer at all — a headless backtest, a CLI, a test calling this
  crate directly — is refused by the store itself, not only by a router it
  has no way to go through. That is why this lives here rather than only
  in `senken-api`. With all nine mutations guarded
  this way, `senken-api`'s router-level `EndpointPermission::Acl` variant
  had nothing left to check that the store did not already check itself,
  so it was removed rather than left as a second, always-redundant gate.

```rust
use senken_identity::{DEFAULT_ADMIN_EMAIL, IdentityStore};

# fn main() -> Result<(), senken_identity::IdentityError> {
# let dir = tempfile::tempdir().unwrap();
let store = IdentityStore::open(dir.path().join("accounts.db"))?;

// First run: the seeded admin has no password yet.
store.set_password(DEFAULT_ADMIN_EMAIL, "correct horse battery staple", None)?;
let (_user_id, token) = store.login(DEFAULT_ADMIN_EMAIL, "correct horse battery staple")?;

let auth = store.resolve_session(token.reveal())?.expect("session was just created");
let page = store.list_users(&auth, 20, 0)?;
assert_eq!(page.total, 1);
# Ok(())
# }
```

See the crate's module-level docs (`cargo doc -p senken-identity --open`)
for the reasoning behind each of these, tied back to the plan sections that
decided them.
