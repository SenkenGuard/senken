// Client-side constants that mirror a server-side value but have no
// corresponding OpenAPI schema to generate from — `GET /api/openapi.json`
// carries request/response shapes, not seed data or bare constants (see
// `routes/login/+page.svelte`'s own note on `DEFAULT_ADMIN_EMAIL` for the
// same reasoning). Before this file existed, `$lib/components/settings/
// settings-api.ts` and the login page each hand-copied their own value —
// the "third coordination gap" cleanup folds them into this
// one, shared home.

/** Mirrors `senken_identity::password::MIN_PASSWORD_LEN`
 * (`crates/identity/src/password.rs`) and "a length floor,
 * no composition rules, no forced rotation." A fast-fail hint for
 * a form only — the server re-checks this independently via
 * `senken_identity`'s own `check_password_len`, so this is never the
 * source of truth. */
export const MIN_PASSWORD_LENGTH = 8;
