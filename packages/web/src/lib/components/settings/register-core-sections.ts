// Registers Senken's own five settings sections — through
// the exact same `registerSettingsSection` a plugin will use later, per
// its "core registers its own sections through the same mechanism
// plugins will use, so the mechanism is exercised from the start rather
// than theorised." There is no separate, privileged "core sections" array
// anywhere; this file is simply the first caller of the registry.
//
// Imported once, for its side effect, by `settings-modal.svelte` — module
// evaluation order in JS means the `registerSettingsSection` calls below
// run exactly once no matter how many times this module is imported.
import UserIcon from '@lucide/svelte/icons/user';
import PaletteIcon from '@lucide/svelte/icons/palette';
import ServerIcon from '@lucide/svelte/icons/server';
import PuzzleIcon from '@lucide/svelte/icons/puzzle';
import ShieldIcon from '@lucide/svelte/icons/shield';
import HardDriveIcon from '@lucide/svelte/icons/hard-drive';
import InfoIcon from '@lucide/svelte/icons/info';

import { registerSettingsSection } from '$lib/state/settings-registry.svelte';

import AccountSection, { searchIndex as accountSearchIndex } from './sections/account-section.svelte';
import AppearanceSection, { searchIndex as appearanceSearchIndex } from './sections/appearance-section.svelte';
import ConnectionSection, { searchIndex as connectionSearchIndex } from './sections/connection-section.svelte';
import PluginsSection, { searchIndex as pluginsSearchIndex } from './sections/plugins-section.svelte';
import AccessSection, { searchIndex as accessSearchIndex } from './sections/access-section.svelte';
import AboutSection, { searchIndex as aboutSearchIndex } from './sections/about-section.svelte';
import StorageSection, { searchIndex as storageSearchIndex } from './sections/storage-section.svelte';

registerSettingsSection({
	id: 'account',
	label: 'Account',
	icon: UserIcon,
	component: AccountSection,
	searchIndex: accountSearchIndex,
	order: 10
});

registerSettingsSection({
	id: 'appearance',
	label: 'Appearance',
	icon: PaletteIcon,
	component: AppearanceSection,
	searchIndex: appearanceSearchIndex,
	order: 20
});

registerSettingsSection({
	id: 'connection',
	label: 'Connection',
	icon: ServerIcon,
	component: ConnectionSection,
	searchIndex: connectionSearchIndex,
	// Choosing which server to talk to is a desktop-app question. A browser
	// tab was served by exactly one server and cannot move to another
	// without navigating away from it, so this section would offer a choice
	// the page cannot honour. The connection's *state* is still visible
	// there, on the top bar's own indicator.
	desktopOnly: true,
	order: 30
});

registerSettingsSection({
	id: 'plugins',
	label: 'Plugins',
	icon: PuzzleIcon,
	component: PluginsSection,
	searchIndex: pluginsSearchIndex,
	order: 40
});

registerSettingsSection({
	id: 'storage',
	label: 'Storage',
	icon: HardDriveIcon,
	component: StorageSection,
	searchIndex: storageSearchIndex,
	// Shown to an account granted storage administration, which is not the
	// same set as the accounts that administer users. Hiding it is still
	// only cosmetic — the endpoints behind it check a real grant on every
	// request.
	requiresAnyResource: ['Storage'],
	order: 45
});

registerSettingsSection({
	id: 'access',
	label: 'Users & Roles',
	icon: ShieldIcon,
	component: AccessSection,
	searchIndex: accessSearchIndex,
	requiresAnyResource: ['User', 'Role'],
	order: 50
});

// Last in the nav, where an About pane conventionally sits.
registerSettingsSection({
	id: 'about',
	label: 'About',
	icon: InfoIcon,
	component: AboutSection,
	searchIndex: aboutSearchIndex,
	order: 60
});
