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

import { registerSettingsSection } from '$lib/state/settings-registry.svelte';

import AccountSection, { searchIndex as accountSearchIndex } from './sections/account-section.svelte';
import AppearanceSection, { searchIndex as appearanceSearchIndex } from './sections/appearance-section.svelte';
import ConnectionSection, { searchIndex as connectionSearchIndex } from './sections/connection-section.svelte';
import PluginsSection, { searchIndex as pluginsSearchIndex } from './sections/plugins-section.svelte';
import AccessSection, { searchIndex as accessSearchIndex } from './sections/access-section.svelte';

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
	id: 'access',
	label: 'Users & Roles',
	icon: ShieldIcon,
	component: AccessSection,
	searchIndex: accessSearchIndex,
	adminOnly: true,
	order: 50
});
