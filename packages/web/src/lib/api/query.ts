// Server-cache wiring ("`@tanstack/svelte-query` 6.1.48… worth the dependency for one reason — the instrument catalog, bar ranges and account lists are all requested from several panels at once, and the alternative is each panel fetching its own copy.") No panel using it yet
// exists yet, but the plumbing —  one
// shared `QueryClient`, mounted once in `AppShell` (see
// `connection-status.svelte`'s sibling wiring in `app-shell.svelte`) — is
// what this stage is responsible for.
//
// Explicitly NOT used for the WebSocket stream (`websocket.ts`,
// `ws-events.svelte.ts`) — B16: "pushed data is not a query."
import { QueryClient, createQuery } from '@tanstack/svelte-query';
import { apiClient } from './client';

export const queryClient = new QueryClient({
	defaultOptions: {
		queries: {
			// A dead connection is exactly what `connectionStore` (not
			// react-query's own retry loop) is responsible for recovering
			// from — `ApiClient.startHeartbeat` already backs off and
			// retries at the transport level, so a query retrying on top of
			// that would double the backoff for no benefit.
			retry: false
		}
	}
});

/** Example/first real consumer of the cache — every panel that wants
 * `/api/health` gets the same request and the same cached result instead
 * of firing its own `fetch`. Follow this shape for the instrument
 * catalog/bar-range/account-list queries Q4 adds. */
export function createHealthQuery() {
	return createQuery(() => ({
		queryKey: ['health'],
		queryFn: () => apiClient.health()
	}));
}
