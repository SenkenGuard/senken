// Pure loopback/HTTPS check backing its UI warning (`servers.svelte.ts`'s
// `isSecureConnection`, `connection-status.svelte`'s "⚠ INSECURE" badge).
// Kept in its own plain (non-rune) module so it can be unit-tested with
// `bun test` directly — every rune-bearing `.svelte.ts` file in this
// package requires Svelte's compiler to even load (`$state` isn't a real
// runtime function outside it), which a plain `bun test` run doesn't
// provide.

const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '[::1]', '::1']);

export function isLoopbackHost(hostname: string): boolean {
	return LOOPBACK_HOSTS.has(hostname) || hostname.endsWith('.localhost');
}

/** `true` when a connection to `rawUrl` is safe from its perspective:
 * loopback (traffic never leaves the machine) or HTTPS (TLS-protected). An
 * unparsable URL is treated as unsafe — this function only says "yes, this
 * is fine," never "I couldn't tell, assume it's fine." */
export function isSecureUrl(rawUrl: string): boolean {
	try {
		const url = new URL(rawUrl);
		return url.protocol === 'https:' || isLoopbackHost(url.hostname);
	} catch {
		return false;
	}
}
