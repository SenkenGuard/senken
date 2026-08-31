// The message a rejected layout mutation should show. Pulled out of
// `workspace-store.svelte.ts` as its own plain function — that module's
// `$state` fields need the Svelte compiler to even load, so this is what
// lets the one genuinely pure decision in `persistAndReload`'s failure path
// be unit-tested with a plain `bun test`.
//
// A 403 here means "you may not edit this layout" specifically, not
// `ForbiddenError`'s generic default message — and, like every 403 in this
// app, it must never be read as a dead session (that already holds because
// `client.ts` never touches the credential for a 403; nothing here needs to
// repeat that, only avoid contradicting it in the text shown).
import { ForbiddenError, getErrorMessage } from '$lib/api/errors';

export function layoutMutationErrorMessage(error: unknown): string {
	return error instanceof ForbiddenError
		? 'You do not have permission to edit this layout.'
		: getErrorMessage(error, 'Could not save that change.');
}
