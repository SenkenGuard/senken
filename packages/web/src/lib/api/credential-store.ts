// Per-server credential persistence.
//
// B11 decides credentials are keyed **by server** — connecting to three
// Senkens means three independent credentials — and that on desktop they
// belong in the OS keyring, never a JSON file. The keyring integration
// itself is out of scope for this stage ("the desktop keyring belongs
// to a later milestone — leave a clean seam, do not build it"), so this
// file defines the seam as an interface and ships exactly one
// implementation: `localStorage`, which is what B2 calls for "in the
// browser."
//
// `ApiClient` and the server-selection store depend on the `CredentialStore`
// interface, not on `LocalStorageCredentialStore` directly, so a future
// `KeyringCredentialStore` (backed by a Tauri command that calls the
// `keyring` crate) can be swapped in for the desktop build without
// touching a call site.
//
// This module exports only the interface and the implementation, never a
// constructed instance. The one instance lives in `session.svelte.ts`,
// unexported, so `set`/`clear` are reachable from nowhere but that file's
// session funnel — there is no `credentialStore` binding anywhere else to
// import. Read access for the rest of the app goes through that module's
// `credentialReader` instead.

export interface CredentialStore {
	/** The stored bearer token for `serverId`, or `null` if none is set. */
	get(serverId: string): string | null;
	/** Persist `token` as the credential for `serverId`. */
	set(serverId: string, token: string): void;
	/** Remove any stored credential for `serverId`. */
	clear(serverId: string): void;
}

const STORAGE_KEY_PREFIX = 'senken.credential.';

/** `localStorage`-backed `CredentialStore`. Guards every access against
 * `localStorage` being unavailable (SSR, privacy mode, disabled storage) so
 * a throw here never takes down the connection layer — a missing
 * credential is a normal, expected state (first run, logged out). */
export class LocalStorageCredentialStore implements CredentialStore {
	get(serverId: string): string | null {
		try {
			return localStorage.getItem(STORAGE_KEY_PREFIX + serverId);
		} catch {
			return null;
		}
	}

	set(serverId: string, token: string): void {
		try {
			localStorage.setItem(STORAGE_KEY_PREFIX + serverId, token);
		} catch {
			// Storage unavailable or full — the credential simply won't
			// survive a reload. Nothing to recover here; the connection
			// layer treats a missing credential the same as a logged-out
			// user.
		}
	}

	clear(serverId: string): void {
		try {
			localStorage.removeItem(STORAGE_KEY_PREFIX + serverId);
		} catch {
			// See `set` above.
		}
	}
}
