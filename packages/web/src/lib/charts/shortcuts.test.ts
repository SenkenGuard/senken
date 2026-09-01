import { describe, expect, test } from 'bun:test';
import { shouldResetActivePaneView, type ShortcutKeyEvent } from './shortcuts';

function altR(overrides: Partial<ShortcutKeyEvent> = {}): ShortcutKeyEvent {
	return { key: 'r', altKey: true, ctrlKey: false, metaKey: false, ...overrides };
}

describe('shouldResetActivePaneView', () => {
	test('fires on a plain Alt+R with no dialog open and no editable focus', () => {
		expect(shouldResetActivePaneView(altR(), null, false)).toBe(true);
	});

	test('is case-insensitive about the key — Shift+Alt+R reports "R"', () => {
		expect(shouldResetActivePaneView(altR({ key: 'R' }), null, false)).toBe(true);
	});

	test('does not fire from inside an input', () => {
		expect(shouldResetActivePaneView(altR(), { tagName: 'INPUT' }, false)).toBe(false);
	});

	test('does not fire from inside a textarea', () => {
		expect(shouldResetActivePaneView(altR(), { tagName: 'TEXTAREA' }, false)).toBe(false);
	});

	test('does not fire from a contenteditable element', () => {
		expect(shouldResetActivePaneView(altR(), { tagName: 'DIV', isContentEditable: true }, false)).toBe(false);
	});

	test('does not fire with Ctrl also held', () => {
		expect(shouldResetActivePaneView(altR({ ctrlKey: true }), null, false)).toBe(false);
	});

	test('does not fire with Meta also held', () => {
		expect(shouldResetActivePaneView(altR({ metaKey: true }), null, false)).toBe(false);
	});

	test('does not fire without Alt held', () => {
		expect(shouldResetActivePaneView(altR({ altKey: false }), null, false)).toBe(false);
	});

	test('does not fire for a different key', () => {
		expect(shouldResetActivePaneView(altR({ key: 't' }), null, false)).toBe(false);
	});

	test('does not fire while a dialog is open, even from a plain button', () => {
		expect(shouldResetActivePaneView(altR(), { tagName: 'BUTTON' }, true)).toBe(false);
	});
});
