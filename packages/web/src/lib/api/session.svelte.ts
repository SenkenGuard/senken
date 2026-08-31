// The session funnel, and the auth gate's mirror of it.
//
// `credential-store.ts` is deliberately plain (a thin `localStorage`
// wrapper, not a rune store — see its own module doc), so nothing there
// re-renders when a credential is set or cleared. The auth gate in
// `app-shell.svelte` needs exactly that signal to decide, on every render,
// whether to show the terminal shell or redirect to `/login` — so this
// file mirrors credential presence into a `$state` rune.
//
// The mirror used to be manually refreshed by every mutator, and it
// drifted — two call sites cleared the credential and never refreshed it,
// so the gate kept reporting "authorized" after sign-out. This closes that
// by construction instead of by convention: the one `CredentialStore`
// instance is constructed here and never exported, so `set`/`clear` are
// reachable from nowhere else in the app. Establishing or ending a session
// means calling `establishSession`/`endSession` below — both wrap
// `SessionFunnel` (`session-funnel.ts`), which updates the rune as part of
// the same call; there is no separate step to forget.
import { activeServer } from './servers.svelte';
import { LocalStorageCredentialStore, type CredentialStore } from './credential-store';
import { resetTransientUi } from '$lib/state/transient-ui';
import { SessionFunnel } from './session-funnel';

const store: CredentialStore = new LocalStorageCredentialStore();

class SessionStore {
	hasCredential = $state(false);
}

export const sessionStore = new SessionStore();

/** Read-only handle onto the credential store, for the one caller
 * (`ApiClient`) that needs to read a token to attach it to a request or to
 * decide what the heartbeat should poll. Typed to `get` alone, so a `.set`
 * or `.clear` written against this binding is a compile error, not a
 * runtime drift. */
export const credentialReader: Pick<CredentialStore, 'get'> = store;

const funnel = new SessionFunnel({
	store,
	activeServerId: () => activeServer().id,
	onPresenceChange: (present) => {
		sessionStore.hasCredential = present;
	},
	resetTransientUi
});

/** Reads the stored credential once, before the gate is first rendered.
 *
 * This cannot happen while this module is being evaluated. `servers.svelte`
 * imports this file, so when it is the one evaluated first, reading
 * `activeServer()` at module scope reaches into a module that has not
 * finished defining its store yet and throws. Priming from a component's
 * setup instead runs after every module in the cycle is initialised, and
 * still before the first render, so the gate never sees a stale `false`. */
export function primeSessionPresence(): void {
	funnel.refreshPresence();
}

/** Re-reads the store for the *currently active* server without mutating
 * anything. Only needed when the active server itself changes — a login,
 * logout, or session expiry should call `establishSession`/`endSession`
 * instead, which refresh as part of the mutation. */
export function refreshSessionPresence(): void {
	funnel.refreshPresence();
}

/** Establishes a session: persists `token` for `serverId` and updates the
 * auth gate in the same call. Call on a successful login. */
export function establishSession(serverId: string, token: string): void {
	funnel.establishSession(serverId, token);
}

/** Ends whatever session `serverId` holds: clears its credential, updates
 * the auth gate, and resets transient UI (the settings modal, the command
 * palette) so nothing from the old session is still on screen the next
 * time someone lands here. Call on logout — success or failure — and on an
 * involuntary session expiry (a 401). */
export function endSession(serverId: string): void {
	funnel.endSession(serverId);
}

/** Clears a server's stored credential without treating it as "my current
 * session ended" — for removing a server entry that may not even be the
 * active one. Still refreshes the gate, since removing the *active* server
 * changes which one `activeServer()` resolves to. */
export function forgetServerCredential(serverId: string): void {
	funnel.forgetServerCredential(serverId);
}
