import { describe, expect, test } from 'bun:test';
import { SessionFunnel } from './session-funnel';
import type { CredentialStore } from './credential-store';

/** In-memory stand-in for `LocalStorageCredentialStore`, keyed the same
 * way — nothing here depends on `localStorage` itself. */
function memoryStore(): CredentialStore {
	const values = new Map<string, string>();
	return {
		get: (id) => values.get(id) ?? null,
		set: (id, token) => void values.set(id, token),
		clear: (id) => void values.delete(id)
	};
}

/** Wires a `SessionFunnel` to plain recorder variables instead of a
 * `$state` rune and the real `resetTransientUi` — exactly the seam
 * `SessionFunnelDeps` exists for. */
function harness(activeId = 'srv-1') {
	const store = memoryStore();
	let presence: boolean | null = null;
	let presenceUpdates = 0;
	let transientUiResets = 0;
	const funnel = new SessionFunnel({
		store,
		activeServerId: () => activeId,
		onPresenceChange: (present) => {
			presence = present;
			presenceUpdates++;
		},
		resetTransientUi: () => transientUiResets++
	});
	return {
		store,
		funnel,
		presence: () => presence,
		presenceUpdates: () => presenceUpdates,
		transientUiResets: () => transientUiResets
	};
}

describe('SessionFunnel — the auth-gate mirror can only drift through this file', () => {
	test('establishSession persists the token and reports presence in the same call', () => {
		const h = harness();
		h.funnel.establishSession('srv-1', 'tok-abc');
		expect(h.store.get('srv-1')).toBe('tok-abc');
		expect(h.presence()).toBe(true);
	});

	// This is the property the whole class exists for: the bug was two
	// call sites clearing the credential without ever telling the gate, so
	// `sessionStore.hasCredential` stayed `true` after a real sign-out.
	// Commenting out the `this.refresh()` line inside `endSession` (in
	// `session-funnel.ts`) reproduces exactly that — this test then fails
	// with `presence() === null` instead of `false`, proving the assertion
	// is not vacuous. Restored after confirming that.
	test('endSession clears the credential and flips the gate off in the same call', () => {
		const h = harness();
		h.funnel.establishSession('srv-1', 'tok-abc');
		h.funnel.endSession('srv-1');
		expect(h.store.get('srv-1')).toBeNull();
		expect(h.presence()).toBe(false);
	});

	test('endSession resets transient UI; establishSession does not', () => {
		const h = harness();
		h.funnel.establishSession('srv-1', 'tok-abc');
		expect(h.transientUiResets()).toBe(0);
		h.funnel.endSession('srv-1');
		expect(h.transientUiResets()).toBe(1);
	});

	test('forgetServerCredential clears the credential and refreshes the gate, but never resets transient UI', () => {
		const h = harness();
		h.funnel.establishSession('srv-1', 'tok-abc');
		h.funnel.forgetServerCredential('srv-1');
		expect(h.store.get('srv-1')).toBeNull();
		expect(h.presence()).toBe(false);
		expect(h.transientUiResets()).toBe(0);
	});

	test('forgetServerCredential on a server that is not active does not flip presence for the active one', () => {
		const h = harness('srv-active');
		h.funnel.establishSession('srv-active', 'tok-active');
		h.funnel.forgetServerCredential('srv-other');
		// The active server's own credential is untouched; refresh() still
		// runs (it always reads the *active* server), so it reports `true`
		// again rather than leaving a stale value.
		expect(h.store.get('srv-active')).toBe('tok-active');
		expect(h.presence()).toBe(true);
	});

	test('refreshPresence re-reads without mutating the store', () => {
		const h = harness();
		h.store.set('srv-1', 'tok-preexisting');
		expect(h.presence()).toBeNull(); // nothing has read it yet
		h.funnel.refreshPresence();
		expect(h.presence()).toBe(true);
		expect(h.store.get('srv-1')).toBe('tok-preexisting');
	});
});
