import { describe, expect, test } from 'bun:test';
import { layoutMutationErrorMessage } from './workspace-error';
import { ForbiddenError, HttpError, NetworkError } from '$lib/api/errors';

describe('layoutMutationErrorMessage — B3\'s "you may not edit this layout" wording', () => {
	test('a rejected layout edit reads as a permission message, not the generic default', () => {
		const message = layoutMutationErrorMessage(new ForbiddenError());
		expect(message).toBe('You do not have permission to edit this layout.');
		// Guards against silently falling back to `ForbiddenError`'s own
		// generic text if this ever gets refactored to just `error.message`.
		expect(message).not.toBe(new ForbiddenError().message);
	});

	test('a network failure falls back to the generic save-failure message', () => {
		const message = layoutMutationErrorMessage(new NetworkError('Could not reach the server.'));
		expect(message).toBe('Could not save that change.');
	});

	test("an HttpError with a server-provided reason surfaces the server's own text", () => {
		const message = layoutMutationErrorMessage(new HttpError('Request failed with 422.', 422, { error: 'layout has no panes' }));
		expect(message).toBe('layout has no panes');
	});

	test('a plain unrecognised error still falls back rather than throwing', () => {
		expect(layoutMutationErrorMessage('not an Error at all')).toBe('Could not save that change.');
	});
});
