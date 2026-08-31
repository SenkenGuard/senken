// The rune-free core of the session funnel: what happens when a session is
// established or ended, independent of how the auth gate re-renders.
//
// Deliberately its own tiny, rune-free class — same reasoning as
// `once-guard.ts` — so the property this exists for ("ending a session
// always updates the gate in the same call, never as a separate step") is
// unit-testable with a plain `bun test`, without needing the Svelte
// compiler for `$state`. `session.svelte.ts` is the only place that
// constructs one, feeding it the real credential store and the rune it
// writes to.
import type { CredentialStore } from './credential-store';

export interface SessionFunnelDeps {
	store: CredentialStore;
	/** The currently active server's id, read fresh on every call — it can
	 * change between one call and the next. */
	activeServerId: () => string;
	/** Invoked with the freshly-read credential presence for the active
	 * server, every time a mutator runs. In production this writes the
	 * `$state` rune the auth gate reads; in a test it can be a plain
	 * variable. */
	onPresenceChange: (present: boolean) => void;
	/** Closes whatever transient UI (the settings modal, the command
	 * palette) a session leaves open. Called only on `endSession` — a
	 * server-credential removal that isn't the current session ending
	 * (`forgetServerCredential`) has nothing to close. */
	resetTransientUi: () => void;
}

export class SessionFunnel {
	constructor(private readonly deps: SessionFunnelDeps) {}

	private refresh(): void {
		this.deps.onPresenceChange(this.deps.store.get(this.deps.activeServerId()) !== null);
	}

	/** Re-reads presence for the active server without mutating anything —
	 * for a server switch, which changes which server is "active" without
	 * touching any credential. */
	refreshPresence(): void {
		this.refresh();
	}

	/** Persists `token` for `serverId` and updates the gate in the same
	 * call. Call on a successful login. */
	establishSession(serverId: string, token: string): void {
		this.deps.store.set(serverId, token);
		this.refresh();
	}

	/** Clears `serverId`'s credential, updates the gate, and resets
	 * transient UI — the one path an actual session ends on: a logout
	 * (success or failure) or an involuntary expiry. */
	endSession(serverId: string): void {
		this.deps.store.clear(serverId);
		this.refresh();
		this.deps.resetTransientUi();
	}

	/** Clears `serverId`'s credential and updates the gate, without treating
	 * it as "my current session ended" — for removing a server entry that
	 * need not be the active one. */
	forgetServerCredential(serverId: string): void {
		this.deps.store.clear(serverId);
		this.refresh();
	}
}
