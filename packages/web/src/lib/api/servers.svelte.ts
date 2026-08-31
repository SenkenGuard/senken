// Server selection and persistence.
//
// B1 is the reason this file exists at all: "the client can point at any
// server, not just the embedded one," and reversing that assumption later
// "touches every call site" — so server identity is a first-class, mutable,
// persisted piece of state from the start, not a hard-coded base URL.
//
// Follows the module-level rune store pattern from `$lib/state/` (see
// `accounts.svelte.ts`) rather than introducing a second convention for
// state that lives in `$lib/api/` instead of `$lib/state/` — `ApiClient`
// owning the connection layer doesn't change how its reactive state is
// expressed.
import { forgetServerCredential } from './session.svelte';
import { isSecureUrl } from './security';

export interface ServerConfig {
	id: string;
	label: string;
	/** Absolute origin (`https://trading.example.com:4206`), or `''` to mean
	 * "this same origin" — the server that served the page itself. `''` is
	 * what makes the default entry work identically in dev (Vite proxies
	 * `/api` to the Rust server, see `vite.config.ts`), in `senken serve`,
	 * and in `senken gui` (the webview's own origin) without knowing the
	 * port in advance. */
	baseUrl: string;
}

/** The server every install starts pointed at: wherever the page came
 * from. Not persisted itself — it's the fallback when no server has been
 * added yet — but it is a normal entry once the user picks it, like any
 * other. */
export const EMBEDDED_SERVER: ServerConfig = {
	id: 'embedded',
	label: 'This device',
	baseUrl: ''
};

const SERVERS_KEY = 'senken.servers';
const ACTIVE_SERVER_KEY = 'senken.activeServerId';

function loadServers(): ServerConfig[] {
	try {
		const raw = localStorage.getItem(SERVERS_KEY);
		if (!raw) return [EMBEDDED_SERVER];
		const parsed: unknown = JSON.parse(raw);
		if (!Array.isArray(parsed) || parsed.length === 0) return [EMBEDDED_SERVER];
		return parsed as ServerConfig[];
	} catch {
		return [EMBEDDED_SERVER];
	}
}

function loadActiveId(): string {
	try {
		return localStorage.getItem(ACTIVE_SERVER_KEY) ?? EMBEDDED_SERVER.id;
	} catch {
		return EMBEDDED_SERVER.id;
	}
}

function persistServers(list: ServerConfig[]): void {
	try {
		localStorage.setItem(SERVERS_KEY, JSON.stringify(list));
	} catch {
		// Best-effort — see `credential-store.ts` for why a storage failure
		// here must not throw.
	}
}

function persistActiveId(id: string): void {
	try {
		localStorage.setItem(ACTIVE_SERVER_KEY, id);
	} catch {
		// See `persistServers` above.
	}
}

class ServersStore {
	list = $state<ServerConfig[]>(loadServers());
	activeId = $state<string>(loadActiveId());
}

export const serversStore = new ServersStore();

/** The currently selected server, or `EMBEDDED_SERVER` if the persisted
 * selection no longer exists (e.g. it was removed on another tab). */
export function activeServer(): ServerConfig {
	return serversStore.list.find((s) => s.id === serversStore.activeId) ?? EMBEDDED_SERVER;
}

/** Add a server to the persisted list and return its assigned id. Does not
 * select it — call `selectServer` to switch the active connection. */
export function addServer(config: Omit<ServerConfig, 'id'>): ServerConfig {
	const server: ServerConfig = { ...config, id: crypto.randomUUID() };
	serversStore.list = [...serversStore.list, server];
	persistServers(serversStore.list);
	return server;
}

/** Remove a server and its stored credential. Refuses to remove the last
 * remaining entry — there must always be somewhere to connect to.
 *
 * Selects the next server *before* forgetting the credential, so if `id`
 * was the active one, the auth-gate mirror `forgetServerCredential`
 * refreshes already reflects the newly active server rather than the one
 * just removed. */
export function removeServer(id: string): void {
	if (serversStore.list.length <= 1) return;
	const wasActive = serversStore.activeId === id;
	serversStore.list = serversStore.list.filter((s) => s.id !== id);
	persistServers(serversStore.list);
	if (wasActive) {
		selectServer(serversStore.list[0].id);
	}
	forgetServerCredential(id);
}

/** Switch the active server. This is the one call that changes which
 * server `ApiClient` talks to —/its done-criterion, no restart is
 * required. */
export function selectServer(id: string): void {
	serversStore.activeId = id;
	persistActiveId(id);
}

/** Resolve a server's base URL to an absolute origin, for the fetch layer
 * and for the loopback/https check below. `''` (the embedded server)
 * resolves to `window.location.origin` — the page's own origin, which is
 * always where the embedded server lives whether that's the Vite dev
 * proxy's origin or the origin `senken serve`/`senken gui` bound. */
export function resolveBaseUrl(server: ServerConfig): string {
	return server.baseUrl === '' ? window.location.origin : server.baseUrl;
}

/** B15: "the client must warn when the chosen server is neither loopback
 * nor `https`." Returns `true` when the connection is safe (loopback, so
 * the traffic never leaves the machine, or TLS-protected); `false` means
 * the UI must show a warning. The actual check is `security.ts`'s
 * `isSecureUrl`, kept pure and unit-tested there. */
export function isSecureConnection(server: ServerConfig): boolean {
	return isSecureUrl(resolveBaseUrl(server));
}
