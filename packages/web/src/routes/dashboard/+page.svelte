<script lang="ts">
	// Dashboard route: real, server-backed workspaces and widget grid — the
	// `packages/web/src/lib/mock/dashboard.ts` shapes (`WidgetInstance`,
	// `Workspace`, `CatalogEntry`) raised to the server rather than kept as
	// UI-only fixtures. `routes/+page.svelte` (the pre-existing local-only
	// demo dashboard) is left exactly as it is — this is a new route, not a
	// replacement of that one.
	import { onMount } from 'svelte';
	import { toast } from 'svelte-sonner';
	import {
		createDashboardWorkspace,
		dashboardWidgetCatalog,
		defaultDashboardWorkspace,
		deleteDashboardWorkspace,
		getDashboardLayout,
		listDashboardWorkspaces,
		renameDashboardWorkspace,
		widgetPluginCatalog,
		type DashboardLayoutDto,
		type DashboardWidgetDefinition,
		type DashboardWorkspaceDto
	} from '$lib/components/dashboard/api';
	import { registerPluginWidgets } from '$lib/components/dashboard/widget-registry';
	import AddWidgetPicker from '$lib/components/dashboard/add-widget-picker.svelte';
	import DashboardGrid from '$lib/components/dashboard/dashboard-grid.svelte';
	import DashboardWorkspaceBar from '$lib/components/dashboard/dashboard-workspace-bar.svelte';
	import WidgetPluginsManager from '$lib/components/dashboard/widget-plugins-manager.svelte';

	let workspaces = $state<DashboardWorkspaceDto[]>([]);
	let activeId = $state('');
	let layout = $state<DashboardLayoutDto | null>(null);
	// The effective catalog: built-in widgets from
	// `GET /api/dashboard/widgets/catalog` plus every widget an active
	// widget plugin package contributes from
	// `GET /api/widget-plugins/catalog` — one array, since
	// `WidgetPluginDefinition` is a structural superset of
	// `DashboardWidgetDefinition` (see that type's own doc comment).
	// Neither `add-widget-picker.svelte` nor `dashboard-grid.svelte` needs
	// to know which half of this array a given entry came from.
	let catalog = $state<DashboardWidgetDefinition[]>([]);
	let loading = $state(true);
	let addWidgetOpen = $state(false);
	let widgetPluginsOpen = $state(false);
	let grid = $state<DashboardGrid | undefined>();

	async function loadWorkspaces() {
		const page = await listDashboardWorkspaces(200, 0);
		workspaces = page.rows;
	}

	async function openWorkspace(id: string) {
		activeId = id;
		layout = await getDashboardLayout(id);
	}

	/** Fetches both halves of the effective catalog and registers the
	 * plugin half's renderers — called on first load and again whenever
	 * the widget plugins manager reports a change (install, enable,
	 * disable, remove, refresh), so a disabled package's widgets fall out
	 * of the picker and placed instances of them become placeholders on
	 * the very next render, and an enabled one's come back.
	 *
	 * The plugin half is fetched independently of the built-in half and
	 * degrades to an empty list on failure rather than failing the whole
	 * dashboard: `widget-plugin-handlers`'s routes are not registered on
	 * every build yet (see that module's own doc comment on the wiring it
	 * still needs), and a server that has not mounted them yet must not
	 * make the built-in catalog — and therefore the whole dashboard —
	 * unreachable. A widget already placed from a plugin that is
	 * unreachable this way still renders as a placeholder, exactly like a
	 * disabled or missing provider always has. */
	async function reloadCatalog() {
		const [builtin, plugins] = await Promise.all([
			dashboardWidgetCatalog(),
			widgetPluginCatalog().catch(() => ({ widgets: [] }))
		]);
		registerPluginWidgets(plugins.widgets);
		catalog = [...builtin.widgets, ...plugins.widgets];
	}

	onMount(async () => {
		try {
			const [{ workspace_id }] = await Promise.all([defaultDashboardWorkspace(), reloadCatalog()]);
			await loadWorkspaces();
			await openWorkspace(workspace_id);
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
			onSelect={openWorkspace}
			onAdd={handleAddWorkspace}
			onOpenAddWidget={() => (addWidgetOpen = true)}
			onOpenWidgetPlugins={() => (widgetPluginsOpen = true)}
			onRename={handleRename}
			onDelete={handleDelete}
		/>

		<div class="min-h-0 flex-1 overflow-auto bg-background p-3">
			{#key layout.workspace.id}
				<DashboardGrid bind:this={grid} {layout} {catalog} onLayoutSaved={handleLayoutSaved} />
			{/key}
		</div>

		<AddWidgetPicker
			open={addWidgetOpen}
			{catalog}
			onClose={() => (addWidgetOpen = false)}
			onPick={(definition) => grid?.addWidget(definition)}
		/>

		<WidgetPluginsManager
			open={widgetPluginsOpen}
			onClose={() => (widgetPluginsOpen = false)}
			onCatalogChanged={() => void reloadCatalog()}
		/>
	{/if}
</div>
