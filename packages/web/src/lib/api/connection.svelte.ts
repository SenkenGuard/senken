// The connection state machine.
//
// "disconnected / connecting / authenticated / reconnecting" — owned by the
// connection layer because both the UI (a status indicator, its insecure
// warning) and the WebSocket layer (which must wait for a live session
// before it can request a ticket) need to read it, and neither should be
// the one deciding what state the *other* observes.
//
// Module-level rune store, same pattern as `$lib/state/*`.

export type ConnectionState = 'disconnected' | 'connecting' | 'authenticated' | 'reconnecting';

class ConnectionStore {
	state = $state<ConnectionState>('disconnected');
	/** Set alongside a transition to `disconnected` when the transition was
	 * caused by an error (a failed health check, a 401), so the UI can show
	 * *why* rather than just *that*. Cleared on every other transition. */
	lastError = $state<string | null>(null);
}

export const connectionStore = new ConnectionStore();

export function setConnectionState(state: ConnectionState, error: string | null = null): void {
	connectionStore.state = state;
	connectionStore.lastError = error;
}
