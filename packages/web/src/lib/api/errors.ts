// Typed transport errors.
//
// `ApiClient` never lets a raw `fetch` rejection or a bare HTTP status code
// escape to a caller — every failure becomes one of these, so a component
// can `catch` and branch on `instanceof` instead of re-deriving "was this a
// 401" from a status number it read off a `Response` itself. That
// re-derivation is exactly the kind of thing B16 exists to centralise.

/** Base class for every error `ApiClient` throws. */
export abstract class ApiError extends Error {}

/** The server could not be reached at all — DNS failure, connection
 * refused, offline, CORS rejection before a status line was ever read.
 * Distinct from `HttpError` because there is no status code to inspect. */
export class NetworkError extends ApiError {
	constructor(message: string, cause?: unknown) {
		super(message, { cause });
		this.name = 'NetworkError';
	}
}

/** `401 Unauthorized` — B16 point 2: the session is gone. `ApiClient`
 * handles clearing the credential and routing to login itself before this
 * ever reaches a caller; a caller only sees this if it chose to inspect the
 * failure reason after the fact (e.g. to show a message before the
 * redirect takes effect). */
export class UnauthorizedError extends ApiError {
	constructor(message = 'Session expired.') {
		super(message);
		this.name = 'UnauthorizedError';
	}
}

/** `403 Forbidden` — B16 point 3: authenticated, but not permitted. Must
 * surface as a message in the caller's UI, never trigger a logout. */
export class ForbiddenError extends ApiError {
	constructor(message = 'You do not have permission to do that.') {
		super(message);
		this.name = 'ForbiddenError';
	}
}

/** Any other non-2xx response. `body` is the parsed JSON body when the
 * response declared one, for callers that want the server's own error
 * message (crates/api's fallback 404 body, for instance). */
export class HttpError extends ApiError {
	constructor(
		message: string,
		readonly status: number,
		readonly body?: unknown
	) {
		super(message);
		this.name = 'HttpError';
	}
}

/** Best-effort human-readable reason, for the connection-state machine's
 * `lastError` (a UI detail, not something a caller branches on) rather than
 * for `instanceof` checks — use the classes above for those. */
export function describeError(error: unknown): string {
	return error instanceof Error ? error.message : 'Unknown connection error.';
}

/** The server's own `ErrorBody.error` text (`crates/api/src/dto.rs`), for a
 * form that wants to show *why* a request failed — e.g. the login page's
 * "no such account or wrong password" / "password must be at least 12
 * characters" messages. Falls back to `fallback` for a `NetworkError` (no
 * response to read a message from) or a body that didn't parse as JSON. */
export function getErrorMessage(error: unknown, fallback: string): string {
	if (error instanceof HttpError && error.body && typeof error.body === 'object' && 'error' in error.body) {
		const message = (error.body as { error?: unknown }).error;
		if (typeof message === 'string' && message.length > 0) return message;
	}
	if (error instanceof ForbiddenError || error instanceof UnauthorizedError) return error.message;
	return fallback;
}

export type ResponseOutcome = 'ok' | 'no-content' | 'unauthorized' | 'forbidden' | 'http-error';

/** Pure classification of a response's status into the branch `ApiClient`
 * (`client.ts`) acts on. Split out so the 401-vs-403-vs-everything-else
 * decision — the one B16 explicitly warns is easy to get backwards — is
 * unit-testable without a real `fetch` or a rune-bearing module. */
export function classifyResponse(response: Pick<Response, 'status' | 'ok'>): ResponseOutcome {
	if (response.status === 401) return 'unauthorized';
	if (response.status === 403) return 'forbidden';
	if (response.status === 204) return 'no-content';
	if (!response.ok) return 'http-error';
	return 'ok';
}
