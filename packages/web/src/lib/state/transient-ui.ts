// Resets the UI chrome that outlives a session because it is module-level
// state (`settingsModal`, `commandPalette`) rather than something that
// unmounts with `AppShell`'s authenticated branch.
//
// Called exactly once, from the session funnel's session-end path
// (`$lib/api/session.svelte.ts`'s `endSession`) — never from an individual
// sign-out control — so the happy path, an error path, and an involuntary
// session expiry all close the same things the same way. Without this, a
// modal left open at the moment a session ends is still open the next time
// someone logs in, sitting on top of a dashboard they haven't seen yet.
import { closeSettings } from './settings.svelte';
import { closeCommand } from './command-palette.svelte';

export function resetTransientUi(): void {
	closeSettings();
	closeCommand();
}
