import { describe, expect, test } from 'bun:test';
import { isDesktopShellMarker } from './shell';

describe('telling the desktop window apart from a browser tab', () => {
	test('recognises every marker the desktop shell stamps', () => {
		// `apps/senken/src/gui.rs` writes one of these two, chosen by
		// platform; both mean the same thing here.
		expect(isDesktopShellMarker('tauri')).toBe(true);
		expect(isDesktopShellMarker('tauri-macos')).toBe(true);
	});

	test('a browser tab, which stamps nothing, is not the desktop shell', () => {
		expect(isDesktopShellMarker(undefined)).toBe(false);
		expect(isDesktopShellMarker(null)).toBe(false);
		expect(isDesktopShellMarker('')).toBe(false);
	});

	test('an unrelated marker is not mistaken for the desktop shell', () => {
		// Falling open would show a browser tab controls it cannot honour,
		// so anything unrecognised has to read as "browser".
		expect(isDesktopShellMarker('electron')).toBe(false);
		expect(isDesktopShellMarker('not-tauri')).toBe(false);
	});
});
