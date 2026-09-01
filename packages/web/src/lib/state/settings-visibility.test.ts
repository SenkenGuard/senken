import { describe, expect, test } from 'bun:test';
import {
	isSettingsSectionVisible,
	type SettingsSectionVisibility,
	type SettingsVisibilityContext
} from './settings-visibility';

function section(flags: SettingsSectionVisibility): SettingsSectionVisibility {
	return flags;
}

const PLAIN_BROWSER: SettingsVisibilityContext = { grantedResources: [], isDesktop: false };
const DESKTOP_ADMIN: SettingsVisibilityContext = {
	grantedResources: ['User', 'Role', 'Storage'],
	isDesktop: true
};

describe('which settings sections are worth showing', () => {
	test('a section with no flags is shown to everyone, everywhere', () => {
		expect(isSettingsSectionVisible(section({}), PLAIN_BROWSER)).toBe(true);
		expect(isSettingsSectionVisible(section({}), DESKTOP_ADMIN)).toBe(true);
	});

	test('a desktop-only section is hidden in a browser tab', () => {
		// Not a permission: a browser tab was served by one server and cannot
		// be pointed at another, so there is no choice to offer.
		const connection = section({ desktopOnly: true });
		expect(isSettingsSectionVisible(connection, PLAIN_BROWSER)).toBe(false);
		expect(
			isSettingsSectionVisible(connection, { grantedResources: [], isDesktop: true })
		).toBe(true);
	});

	test('a restricted section is hidden from an account with no grant on it', () => {
		const access = section({ requiresAnyResource: ['User', 'Role'] });
		expect(isSettingsSectionVisible(access, PLAIN_BROWSER)).toBe(false);
		expect(
			isSettingsSectionVisible(access, { grantedResources: ['Role'], isDesktop: false })
		).toBe(true);
	});

	test('one grant out of the named set is enough', () => {
		const access = section({ requiresAnyResource: ['User', 'Role'] });
		expect(
			isSettingsSectionVisible(access, { grantedResources: ['User'], isDesktop: false })
		).toBe(true);
	});

	test('a grant on some other resource does not open an unrelated section', () => {
		// The bug a single "is an admin" flag caused: an account granted
		// users would have been shown storage, and one granted storage would
		// have been refused it.
		const storage = section({ requiresAnyResource: ['Storage'] });
		expect(
			isSettingsSectionVisible(storage, { grantedResources: ['User', 'Role'], isDesktop: false })
		).toBe(false);
		expect(
			isSettingsSectionVisible(storage, { grantedResources: ['Storage'], isDesktop: false })
		).toBe(true);
	});

	test('both rules must be satisfied, not either', () => {
		const both = section({ requiresAnyResource: ['Storage'], desktopOnly: true });
		expect(
			isSettingsSectionVisible(both, { grantedResources: ['Storage'], isDesktop: false })
		).toBe(false);
		expect(isSettingsSectionVisible(both, { grantedResources: [], isDesktop: true })).toBe(false);
		expect(isSettingsSectionVisible(both, DESKTOP_ADMIN)).toBe(true);
	});
});
