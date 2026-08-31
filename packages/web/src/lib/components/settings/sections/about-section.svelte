<script module lang="ts">
	import type { SettingsSearchEntry } from '$lib/state/settings-registry.svelte';

	/** Static text index for cross-section search — kept separate from the
	 * component below because search must not have to mount every section to
	 * know what it contains. See `settings-registry.svelte.ts`'s
	 * `SettingsSearchEntry` doc comment. */
	export const searchIndex: SettingsSearchEntry[] = [
		{
			groupHeading: 'About',
			rowId: 'app-version',
			rowLabel: 'Version',
			rowDescription: 'The version of the Senken server this device is connected to.'
		},
		{
			groupHeading: 'About',
			rowId: 'app-license',
			rowLabel: 'License',
			rowDescription: 'The licence this software is distributed under.'
		}
	];
</script>

<script lang="ts">
	// About — the version this device is actually talking to.
	//
	// The version is read from `GET /api/health`, which reports the running
	// binary's own `CARGO_PKG_VERSION`. It is deliberately not a constant
	// baked into the web bundle: a browser can be pointed at any server in
	// `serversStore`, and a bundle-baked number would report whatever
	// version the *page* was built from rather than the one answering its
	// requests — which is exactly wrong the moment those differ.
	import { activeServer } from '$lib/api/servers.svelte';
	import { apiClient } from '$lib/api/client';
	import SettingsGroup from '../settings-group.svelte';

	/** `null` until the first answer arrives; an `Error` state is kept
	 * distinct from "not yet asked" so the row never shows a stale dash as
	 * though it were an answer. */
	let version = $state<string | null>(null);
	let failed = $state(false);

	$effect(() => {
		// Re-reads whenever the selected server changes, so switching
		// servers in the Connection section updates this without a reload.
		const serverId = activeServer().id;
		let current = true;
		version = null;
		failed = false;

		apiClient
			.health()
			.then((health) => {
				if (!current) return;
				version = health.version;
			})
			.catch(() => {
				if (!current) return;
				failed = true;
			});

		void serverId;
		return () => {
			current = false;
		};
	});
</script>

{#snippet versionControl()}
	<span class="font-mono text-[12px] text-foreground tabular-nums">
		{#if version !== null}
			v{version}
		{:else if failed}
			<span class="text-dim2">Unavailable</span>
		{:else}
			<span class="text-dim2">Checking…</span>
		{/if}
	</span>
{/snippet}

{#snippet licenseControl()}
	<span class="font-mono text-[12px] text-dim2">LGPL-3.0-only</span>
{/snippet}

<div class="flex flex-col gap-6">
	<SettingsGroup
		group={{
			id: 'about',
			heading: 'About',
			description: 'What this device is connected to, and under what terms it is distributed.',
			rows: [
				{
					id: 'app-version',
					label: 'Version',
					description: 'The version of the Senken server this device is connected to.',
					control: versionControl
				},
				{
					id: 'app-license',
					label: 'License',
					description: 'The licence this software is distributed under.',
					control: licenseControl
				}
			]
		}}
	/>
</div>
