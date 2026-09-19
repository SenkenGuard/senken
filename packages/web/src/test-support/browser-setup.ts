import { beforeAll } from 'vitest';

/** Component lifecycle observations (a dialog unmounting, an overlay
 * clearing, a `body` lock lifting) mean nothing if the browser never
 * paints. Three QA sessions on 2026-09-03 were fooled by a hidden pane:
 * `requestAnimationFrame` never fired, so bits-ui's own unmount gate
 * (`internal/animations-complete.js`, which waits on one) never ran, and a
 * closed dialog looked stuck open forever. This probe refuses that
 * environment outright instead of tolerating it. */
beforeAll(async () => {
	if (document.visibilityState !== 'visible') {
		throw new Error(`browser tests need a visible document, got ${document.visibilityState}`);
	}
	const fired = await Promise.race([
		new Promise<boolean>((resolve) => requestAnimationFrame(() => resolve(true))),
		new Promise<boolean>((resolve) => setTimeout(() => resolve(false), 500))
	]);
	if (!fired) throw new Error('requestAnimationFrame did not fire within 500 ms; refusing to run');
});
