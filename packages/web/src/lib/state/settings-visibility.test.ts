import { describe, expect, test } from 'bun:test';
import {
	isSettingsSectionVisible,
	type SettingsSectionVisibility,
	type SettingsVisibilityContext
} from './settings-visibility';

function section(flags: SettingsSectionVisibility): SettingsSectionVisibility {
	return flags;
}

const BROWSER_USER: SettingsVisibilityContext = { isAdmin: false, isDesktop: false };
const DESKTOP_ADMIN: SettingsVisibilityContext = { isAdmin: true, isDesktop: true };

describe('which settings sections are worth showing', () => {
	test('a section with no flags is shown to everyone, everywhere', () => {
		expect(isSettingsSectionVisible(section({}), BROWSER_USER)).toBe(true);
		expect(isSettingsSectionVisible(section({}), DESKTOP_ADMIN)).toBe(true);
	});

	test('a desktop-only section is hidden in a browser tab', () => {
		// Not a permission: a browser tab was served by one server and cannot
		// be pointed at another, so there is no choice to offer.
		const connection = section({ desktopOnly: true });
		expect(isSettingsSectionVisible(connection, BROWSER_USER)).toBe(false);
		expect(isSettingsSectionVisible(connection, { isAdmin: false, isDesktop: true })).toBe(true);
	});

	test('an admin-only section is hidden from a non-admin', () => {
		const access = section({ adminOnly: true });
		expect(isSettingsSectionVisible(access, BROWSER_USER)).toBe(false);
		expect(isSettingsSectionVisible(access, { isAdmin: true, isDesktop: false })).toBe(true);
	});

	test('both flags must be satisfied, not either', () => {
		const both = section({ adminOnly: true, desktopOnly: true });
		expect(isSettingsSectionVisible(both, { isAdmin: true, isDesktop: false })).toBe(false);
		expect(isSettingsSectionVisible(both, { isAdmin: false, isDesktop: true })).toBe(false);
		expect(isSettingsSectionVisible(both, DESKTOP_ADMIN)).toBe(true);
	});
});
