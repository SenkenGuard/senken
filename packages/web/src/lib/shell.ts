// Which shell the page is running in: the desktop window, or a browser tab.
//
// `apps/senken/src/gui.rs` sets `document.documentElement.dataset.shell` to
// `'tauri'` (or `'tauri-macos'`) through an initialization script before the
// page's own code runs. A browser never has it. `layout.css` already keys
// its desktop-only rules off the same attribute; this module is the same
// fact read from script, for the choices CSS cannot make — whether a
// control is rendered at all, rather than how it looks.
//
// Read from the DOM rather than a build flag because one build serves both:
// `senken serve` and `senken gui` ship the identical bundle, so there is no
// compile-time constant that could tell them apart.

/** The value `gui.rs` stamps, and the prefix every variant of it shares. */
const DESKTOP_SHELL_PREFIX = 'tauri';

/** Whether `value` — the root element's `data-shell` attribute — marks the
 * desktop shell. Split out from the DOM read below so the rule is testable
 * without a document, the same way `classifyResponse` is testable without a
 * `fetch`. An absent attribute is a browser, which is the safe default: a
 * control hidden in the desktop shell is a missing feature, while a
 * server-switching control shown in a browser tab offers something the page
 * cannot honour (a browser tab is served *by* one server and cannot move to
 * another without navigating away from it). */
export function isDesktopShellMarker(value: string | null | undefined): boolean {
	return typeof value === 'string' && value.startsWith(DESKTOP_SHELL_PREFIX);
}

/** Whether this page is running inside the desktop window.
 *
 * Not reactive, and does not need to be: the shell is stamped before the
 * app's first render and cannot change while the page is alive. Guarded for
 * a missing `document` so importing this module from a unit test is safe.
 */
export function isDesktopShell(): boolean {
	if (typeof document === 'undefined') return false;
	return isDesktopShellMarker(document.documentElement.dataset.shell);
}
