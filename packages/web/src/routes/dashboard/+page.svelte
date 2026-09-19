<script lang="ts">
	// Dashboard route: real, server-backed workspaces and widget grid.
	// `routes/+page.ts` redirects `/` here — this is the only dashboard the
	// app renders now, not a second one alongside a local-only demo.
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import {
		createDashboardWorkspace,
		defaultDashboardWorkspace,
		deleteDashboardWorkspace,
		getDashboardLayout,
		listDashboardWorkspaces,
		renameDashboardWorkspace,
		type DashboardLayoutDto,
		type DashboardWorkspaceDto
	} from '$lib/components/dashboard/api';
	import { widgetCatalog, refreshWidgetCatalog } from '$lib/components/dashboard/widget-catalog.svelte';
	import { activeServer } from '$lib/api/servers.svelte';
	import AddWidgetPicker from '$lib/components/dashboard/add-widget-picker.svelte';
	import DashboardGrid from '$lib/components/dashboard/dashboard-grid.svelte';
	import DashboardWorkspaceBar from '$lib/components/dashboard/dashboard-workspace-bar.svelte';

	let workspaces = $state<DashboardWorkspaceDto[]>([]);
	let activeId = $state('');
	let layout = $state<DashboardLayoutDto | null>(null);
	let loading = $state(true);
	let addWidgetOpen = $state(false);
	let grid = $state<DashboardGrid | undefined>();
	// Tracks `grid`'s own `widgets` even though it is declared in that
	// child component — Svelte's reactivity graph is not scoped by
	// component boundary, only by which `$state` a read actually touches.
	const placedTypeIds = $derived(grid?.placedWidgetTypeIds() ?? new Set<string>());
	const activeWidgetCount = $derived(grid?.widgetCount() ?? 0);

	async function loadWorkspaces() {
		const page = await listDashboardWorkspaces(200, 0);
		workspaces = page.rows;
	}

	/** Per-server, so switching between two attached Senken servers
	 * (`servers.svelte.ts`) never opens one server's last workspace against
	 * another's workspace ids. Client-side only: this is a one-user,
	 * one-browser convenience for the MVP, not a synced preference — the
	 * server-side equivalent waits on a schema that owns it. */
	function lastWorkspaceStorageKey(): string {
		return `senken.dashboard.lastWorkspace.${activeServer().id}`;
	}

	function readLastWorkspaceId(): string | null {
		try {
			return localStorage.getItem(lastWorkspaceStorageKey());
		} catch {
			// A private/incognito webview can throw on any `localStorage`
			// access at all — falling back to "no remembered workspace" is
			// exactly what a first-ever visit already does.
			return null;
		}
	}

	function rememberWorkspaceId(id: string): void {
		try {
			localStorage.setItem(lastWorkspaceStorageKey(), id);
		} catch {
			// Same as above: remembering is a convenience, never a
			// requirement for the dashboard to work.
		}
	}

	async function openWorkspace(id: string) {
		activeId = id;
		layout = await getDashboardLayout(id);
		rememberWorkspaceId(id);
	}

	/** Refetches the effective catalog before the picker opens, so
	 * installing, enabling, disabling or removing a widget plugin package
	 * from Settings → Plugins in another tab is reflected the next time
	 * *this* one asks, rather than only on a full reload. The dashboard's
	 * own placed widgets react without this: `plugins-section.svelte`
	 * calls `refreshWidgetCatalog()` itself the moment such an action there
	 * succeeds (see `widget-catalog.svelte.ts`'s own doc comment), which
	 * `dashboard-grid.svelte`'s `$derived` catalog lookup picks up
	 * immediately — this call only covers the picker's own snapshot. */
	async function openAddWidget() {
		await refreshWidgetCatalog();
		addWidgetOpen = true;
	}

	onMount(async () => {
		try {
			await Promise.all([loadWorkspaces(), refreshWidgetCatalog()]);
			const remembered = readLastWorkspaceId();
			const target =
				remembered && workspaces.some((w) => w.id === remembered)
					? remembered
					: (await defaultDashboardWorkspace()).workspace_id;
			await openWorkspace(target);
		} catch {
			toast.error('Could not load the dashboard.');
		} finally {
			loading = false;
		}
	});

	async function handleAddWorkspace() {
		const name = `Workspace ${workspaces.length + 1}`;
		const { id } = await createDashboardWorkspace(name);
		await loadWorkspaces();
		await openWorkspace(id);
	}

	async function handleRename(id: string, newName: string) {
		await renameDashboardWorkspace(id, newName);
		await loadWorkspaces();
	}

	async function handleDelete(id: string) {
		if (workspaces.length < 2) {
			toast.error('At least one dashboard workspace must remain.');
			return;
		}
		await deleteDashboardWorkspace(id);
		await loadWorkspaces();
		// The deleted workspace can never be `activeId` afterward — fall
		// back to whichever workspace is now first, the same "default on
		// open" rule a fresh visit would apply.
		if (activeId === id) {
			const { workspace_id } = await defaultDashboardWorkspace();
			await openWorkspace(workspace_id);
		}
	}

	function handleLayoutSaved(saved: DashboardLayoutDto) {
		workspaces = workspaces.map((w) => (w.id === saved.workspace.id ? saved.workspace : w));
	}
</script>

<div class="flex min-h-0 flex-1 flex-col">
	{#if loading}
		<div class="flex flex-1 items-center justify-center font-mono text-[10px] tracking-[0.14em] text-dim uppercase">
			Loading dashboard…
		</div>
	{:else if layout}
		<DashboardWorkspaceBar
			{workspaces}
			{activeId}
			{activeWidgetCount}
			onSelect={openWorkspace}
			onAdd={handleAddWorkspace}
			onOpenAddWidget={() => void openAddWidget()}
			onRename={handleRename}
			onDelete={handleDelete}
		/>

		<div class="min-h-0 flex-1 overflow-auto bg-background p-3">
			{#key layout.workspace.id}
				<DashboardGrid bind:this={grid} {layout} catalog={widgetCatalog.definitions} onLayoutSaved={handleLayoutSaved} />
			{/key}
		</div>

		<AddWidgetPicker
			open={addWidgetOpen}
			catalog={widgetCatalog.definitions}
			{placedTypeIds}
			onClose={() => (addWidgetOpen = false)}
			onPick={(definition) => grid?.addWidget(definition)}
		/>
	{/if}
</div>
