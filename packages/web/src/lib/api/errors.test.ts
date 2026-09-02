import { describe, expect, test } from 'bun:test';
import {
	ApiError,
	ForbiddenError,
	HttpError,
	NetworkError,
	UnauthorizedError,
	classifyResponse,
	describeError,
	getErrorMessage,
	errorBodyMessage
} from './errors';

function fakeResponse(status: number): Pick<Response, 'status' | 'ok'> {
	return { status, ok: status >= 200 && status < 300 };
}

describe('classifyResponse — 401 and 403 must never be confused', () => {
	test('401 is unauthorized', () => {
		expect(classifyResponse(fakeResponse(401))).toBe('unauthorized');
	});

	test('403 is forbidden, distinctly from 401', () => {
		expect(classifyResponse(fakeResponse(403))).toBe('forbidden');
	});

	test('204 is no-content', () => {
		expect(classifyResponse(fakeResponse(204))).toBe('no-content');
	});

	test('200 is ok', () => {
		expect(classifyResponse(fakeResponse(200))).toBe('ok');
	});

	test('other non-2xx (e.g. 500) is a generic http-error, not mistaken for 401/403', () => {
		expect(classifyResponse(fakeResponse(500))).toBe('http-error');
		expect(classifyResponse(fakeResponse(404))).toBe('http-error');
	});
});

describe('error class hierarchy', () => {
	test('every typed error is an ApiError, so a caller can catch broadly or narrowly', () => {
		expect(new UnauthorizedError()).toBeInstanceOf(ApiError);
		expect(new ForbiddenError()).toBeInstanceOf(ApiError);
		expect(new NetworkError('offline')).toBeInstanceOf(ApiError);
		expect(new HttpError('bad', 500)).toBeInstanceOf(ApiError);
	});

	test('UnauthorizedError and ForbiddenError are never the same class', () => {
		expect(new UnauthorizedError()).not.toBeInstanceOf(ForbiddenError);
		expect(new ForbiddenError()).not.toBeInstanceOf(UnauthorizedError);
	});

	test('HttpError carries the status and parsed body', () => {
		const error = new HttpError('failed', 404, { error: 'not found' });
		expect(error.status).toBe(404);
		expect(error.body).toEqual({ error: 'not found' });
	});
});

describe('describeError', () => {
	test('uses the Error message when available', () => {
		expect(describeError(new Error('boom'))).toBe('boom');
	});

	test('falls back for a non-Error throw', () => {
		expect(describeError('a string was thrown')).toBe('Unknown connection error.');
	});
});

describe('getErrorMessage — surfacing the server\'s own ErrorBody.error', () => {
	test('reads the message out of an HttpError body shaped like crates/api/src/dto.rs ErrorBody', () => {
		const error = new HttpError('failed', 400, { error: 'password must be at least 12 characters' });
		expect(getErrorMessage(error, 'fallback')).toBe('password must be at least 12 characters');
	});

	test('falls back when the body is not JSON (e.g. a plain-text 404 from a proxy)', () => {
		const error = new HttpError('failed', 404, 'not found');
		expect(getErrorMessage(error, 'fallback')).toBe('fallback');
	});

	test('falls back when there is no body at all', () => {
		const error = new HttpError('failed', 500, undefined);
		expect(getErrorMessage(error, 'fallback')).toBe('fallback');
	});

	test('a NetworkError has no response to read a message from, so it always falls back', () => {
		expect(getErrorMessage(new NetworkError('offline'), 'fallback')).toBe('fallback');
	});

	test("uses ForbiddenError's own message — B16 point 3: never treated as a login problem", () => {
		expect(getErrorMessage(new ForbiddenError('you do not have permission to do that'), 'fallback')).toBe(
			'you do not have permission to do that'
		);
	});

	test("uses UnauthorizedError's own message", () => {
		expect(getErrorMessage(new UnauthorizedError('Session expired.'), 'fallback')).toBe('Session expired.');
	});
});

describe('the trade UI must read the server\'s own sentence, not describeError\'s generic one', () => {
	test('on the reproduced "no open position" failure, describeError shows only the URL and status', () => {
		// The literal failure this defect was reproduced against: closing an
		// already-closed position returned `400
		// {"error":"no open position for okx-spot:BTCUSDT"}`, and the close
		// dialog showed `describeError`'s generic "Request to <url> failed
		// with 400." instead of the server's own sentence.
		const error = new HttpError('Request to /api/trade/accounts/acct-1/close failed with 400.', 400, {
			error: 'no open position for okx-spot:BTCUSDT'
		});
		expect(describeError(error)).toBe('Request to /api/trade/accounts/acct-1/close failed with 400.');
		expect(getErrorMessage(error, 'Could not close this position.')).toBe(
			'no open position for okx-spot:BTCUSDT'
		);
	});
});

describe('a rejected login must not read as an expired session', () => {
	test("the server's own wording survives into an UnauthorizedError", () => {
		// A wrong password is a 401 like any other, but it answers a
		// different question — and the default message answers the other one.
		const message = errorBodyMessage({ error: 'that email and password do not match an account' });
		expect(message).toBe('that email and password do not match an account');
		expect(getErrorMessage(new UnauthorizedError(message ?? undefined), 'fallback')).toBe(
			'that email and password do not match an account'
		);
	});

	test('a 401 with nothing readable in it still falls back to the session message', () => {
		// Every endpoint that needs a session answers 401 the same way, and
		// there "your session ended" is the correct thing to say.
		expect(errorBodyMessage(undefined)).toBeNull();
		expect(errorBodyMessage('unauthorized')).toBeNull();
		expect(errorBodyMessage({})).toBeNull();
		expect(errorBodyMessage({ error: '' })).toBeNull();
		expect(errorBodyMessage({ error: 42 })).toBeNull();
		expect(getErrorMessage(new UnauthorizedError(), 'fallback')).toBe('Session expired.');
	});
});
