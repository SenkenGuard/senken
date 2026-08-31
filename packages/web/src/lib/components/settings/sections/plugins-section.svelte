<script module lang="ts">
	import type { SettingsSearchEntry } from '$lib/state/settings-registry.svelte';

	export const searchIndex: SettingsSearchEntry[] = [
		{
			groupHeading: 'Plugins',
			rowId: 'no-plugin-sections',
			rowLabel: 'Plugin settings',
			rowDescription: 'Settings contributed by installed plugins appear here.'
		}
	];
</script>

<script lang="ts">
	// Plugins section. Registered so the section exists in
	// the nav from day one — its whole point is that Core "registers its
	// own sections through the same mechanism plugins will use, so the
	// mechanism is exercised from the start" — but there is nothing real to
	// show inside it yet:
	//
	// - The manifest/namespace/registration *contract* exists
	//   for plugin permissions (`crates/plugin/src/lib.rs`), but nothing
	//   registers a plugin *settings section* yet — the
	//   brief lists that as out of scope ("Plugin-contributed sections —
	//   Q7 built the permission declaration; nothing registers UI sections
	//   yet").
	// - `crates/api` has no endpoint listing which plugins a given running
	//   server has compiled in (plugins are compiled-in and activated by
	//   `RuntimeBuilder` Part D — "native plugins cannot be
	//   scoped to one user" — with no HTTP surface over that list), so
	//   there is no live data this section could show even for a read-only
	//   "installed plugins" list without guessing at a server's build.
	//
	// This is therefore an honest empty state, not a stub hiding missing
	// wiring — the moment a plugin registers a section through
	// `registerSettingsSection` (`$lib/state/settings-registry.svelte.ts`),
	// it appears in the nav next to this one automatically.
	import PuzzleIcon from '@lucide/svelte/icons/puzzle';
</script>

<div class="flex flex-col items-center justify-center gap-3 py-16 text-center">
	<PuzzleIcon class="size-8 text-dim" />
	<p class="max-w-sm text-[13px] text-dim2">
		No plugin has registered a settings section yet. When one does, through the same registry
		this modal's own sections use, it will appear in the list on the left.
	</p>
</div>
