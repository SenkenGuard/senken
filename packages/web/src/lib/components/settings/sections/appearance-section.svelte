<script module lang="ts">
	import type { SettingsSearchEntry } from '$lib/state/settings-registry.svelte';

	/** Static text index for cross-section search — kept separate
	 * from the component below because search must not have to mount every
	 * section to know what it contains. See
	 * `settings-registry.svelte.ts`'s `SettingsSearchEntry` doc comment. */
	export const searchIndex: SettingsSearchEntry[] = [
		{
			groupHeading: 'Theme',
			rowId: 'theme-mode',
			rowLabel: 'Appearance',
			rowDescription: 'Light, dark, or match the operating system.'
		}
	];
</script>

<script lang="ts">
	// Appearance settings — the one section with no server round trip at
	// all: theme is a purely client-side preference `mode-watcher` already
	// persists to `localStorage` (see `routes/+layout.svelte`'s
	// `<ModeWatcher defaultMode="dark" />`). Reusing `userPrefersMode` /
	// `setMode` here rather than introducing a second store keeps this
	// section and `nav-rail.svelte`'s theme toggle reading and writing the
	// exact same state.
	import { userPrefersMode, setMode } from 'mode-watcher';
	import SettingsGroup from '../settings-group.svelte';
	import * as NativeSelect from '$lib/components/ui/native-select/index.js';

	// `mode-watcher`'s own `Mode` type isn't part of its public export list
	// (only `SystemModeValue`/`UserPrefersMode`/`SystemPrefersMode` are, per
	// its `index.d.ts`) — this mirrors `dist/modes.js`'s
	// `["dark", "light", "system"]` instead of reaching for an unexported
	// type.
	type ThemeMode = 'dark' | 'light' | 'system';

	// The app's own first-run default (`<ModeWatcher defaultMode="dark" />`)
	// is this row's "default" for the changed/reset affordance — resetting
	// this row should land where a fresh install starts, not on the
	// library's own unrelated `"system"` default.
	const DEFAULT_MODE: ThemeMode = 'dark';

	function onModeChange(event: Event) {
		setMode((event.currentTarget as HTMLSelectElement).value as ThemeMode);
	}
</script>

{#snippet themeControl()}
	<NativeSelect.Root value={userPrefersMode.current} onchange={onModeChange} class="w-[160px]">
		<NativeSelect.Option value="system">System</NativeSelect.Option>
		<NativeSelect.Option value="light">Light</NativeSelect.Option>
		<NativeSelect.Option value="dark">Dark</NativeSelect.Option>
	</NativeSelect.Root>
{/snippet}

<div class="flex flex-col gap-6">
	<SettingsGroup
		group={{
			id: 'theme',
			heading: 'Theme',
			description: 'Controls the color scheme used across the whole app.',
			rows: [
				{
					id: 'theme-mode',
					label: 'Appearance',
					description: 'Light, dark, or match the operating system.',
					control: themeControl,
					changed: () => userPrefersMode.current !== DEFAULT_MODE,
					onReset: () => setMode(DEFAULT_MODE)
				}
			]
		}}
	/>
</div>
