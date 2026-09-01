// Pure "should this keyboard shortcut fire" logic, kept separate from the
// `keydown` listener itself so it is testable without a DOM — the bun:test
// runtime this file's own tests run under has no `HTMLElement` at all, which
// is also why the target this checks is a small duck-typed shape rather than
// a real element type.

/** The subset of `KeyboardEvent` this module reads. */
export interface ShortcutKeyEvent {
	key: string;
	altKey: boolean;
	ctrlKey: boolean;
	metaKey: boolean;
}

/** The subset of `EventTarget`/`HTMLElement` this module reads. */
export interface ShortcutTarget {
	tagName?: string;
	isContentEditable?: boolean;
}

const EDITABLE_TAGS = new Set(['INPUT', 'TEXTAREA']);

function isEditableTarget(target: ShortcutTarget | null | undefined): boolean {
	if (!target) return false;
	if (target.tagName && EDITABLE_TAGS.has(target.tagName)) return true;
	return target.isContentEditable === true;
}

/** Whether an `Alt+R` keydown should reset the active pane's chart view (the
 * context menu's "RESET CHART VIEW · ALT R" item). Ignored while the user is
 * typing — an `<input>`, a `<textarea>`, or any `contenteditable` element —
 * and while a dialog sits over the chart, so a user setting an indicator
 * period does not have the chart jump under them. Case-insensitive on the
 * key itself, since a held Shift reports `"R"` rather than `"r"`. */
export function shouldResetActivePaneView(
	event: ShortcutKeyEvent,
	target: ShortcutTarget | null | undefined,
	dialogOpen: boolean
): boolean {
	if (dialogOpen) return false;
	if (event.ctrlKey || event.metaKey) return false;
	if (!event.altKey) return false;
	if (event.key.toLowerCase() !== 'r') return false;
	return !isEditableTarget(target);
}
