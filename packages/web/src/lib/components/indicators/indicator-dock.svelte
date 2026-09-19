<script lang="ts">
	// The indicator-authoring dock — TradingView's Pine Editor is the shape
	// CEO pointed at: one panel that owns every authoring action (New, Save,
	// Load, Rename, Delete, Add to chart), masking through popups rather than
	// a tab buried in the add-indicator picker. Wraps the charts route's own
	// content in a vertical `PaneGroup` so the dock can be dragged open by
	// height and collapses to nothing rather than being unmounted — the
	// chart panes underneath keep their real DOM nodes and just resize
	// (`chart-pane.svelte`'s own `ResizeObserver` already reacts to that),
	// which is what keeps a lightweight-charts instance from having to be
	// torn down and rebuilt every time the dock opens or closes.
	//
	// All state (`open`, the draft source, dirty-tracking) lives in
	// `indicator-editor.svelte.ts`, a module-level store — not here — so
	// switching chart panes, or closing and reopening this dock, never loses
	// whatever is currently in the editor (034's "kode di editor tidak
	// hilang" requirement).
	import * as Resizable from '$lib/components/ui/resizable/index.js';
	import ConfirmDialog from '$lib/components/ui/confirm-dialog.svelte';
	import { toast } from 'svelte-sonner';
	import type { Snippet } from 'svelte';
	import type { PaneAPI } from 'paneforge';
	import {
		indicatorEditor,
		loadIndicatorList,
		loadToolchainStatus,
		requestSelect,
		confirmDiscardAndSelect,
		cancelDiscard,
		setDraftSource,
		createIndicator,
		saveIndicator,
		renameIndicator,
		deleteIndicator,
		addActiveIndicatorToChart,
		toggleDock
	} from '$lib/charts/indicator-editor.svelte';
	import IndicatorList from './indicator-list.svelte';
	import IndicatorSourceEditor from './indicator-source-editor.svelte';
	import IndicatorToolbar from './indicator-toolbar.svelte';
	import NewIndicatorDialog from './new-indicator-dialog.svelte';
	import RenameIndicatorDialog from './rename-indicator-dialog.svelte';
	import PanePickerDialog from './pane-picker-dialog.svelte';
	import type { IndicatorTemplateId } from './templates';

	let { paneCount, children }: { paneCount: number; children: Snippet } = $props();

	let dockPane = $state<PaneAPI | null>(null);
	let creatingNew = $state(false);
	let newDialogOpen = $state(false);
	let renameDialogOpen = $state(false);
	let deleteConfirmOpen = $state(false);
	let panePickerOpen = $state(false);

	let mounted = false;
	$effect(() => {
		if (mounted) return;
		mounted = true;
		void loadIndicatorList();
		void loadToolchainStatus();
	});

	/** The dock's own default open height, in percent of the pane group —
	 * only used the first time it opens in a session; after that,
	 * `autoSaveId` below remembers whatever height the reader dragged it
	 * to. */
	const DEFAULT_DOCK_SIZE = 32;

	// The dock's own open/closed flag lives in the store (so `Alt+I` and the
	// toolbar button in `+page.svelte` can toggle it too); this effect is
	// what actually drives the paneforge pane in response, in either
	// direction, whichever side changed it. `resize` (not `expand`/
	// `collapse`) deliberately: `expand()` restores whatever size was
	// recorded the last time *this pane* called `collapse()` itself, which
	// is `undefined` the first time the dock ever opens — falling back to
	// `minSize`, which this dock also sets to a real height (`18`) so that
	// fallback is at least usable, but `resize` is unambiguous either way.
	$effect(() => {
		if (indicatorEditor.open) {
			if ((dockPane?.getSize() ?? 0) <= 0) dockPane?.resize(DEFAULT_DOCK_SIZE);
		} else {
			dockPane?.resize(0);
		}
	});

	const activeItem = $derived(indicatorEditor.list.find((r) => r.id === indicatorEditor.activeId) ?? null);
	const hasEverCompiled = $derived(indicatorEditor.list.some((r) => r.compiled));

	function handleNew() {
		newDialogOpen = true;
	}

	async function handleCreate(title: string, template: IndicatorTemplateId) {
		creatingNew = true;
		try {
			await createIndicator(title, template);
			newDialogOpen = false;
		} finally {
			creatingNew = false;
		}
	}

	async function handleAddToChart() {
		if (paneCount > 1) {
			panePickerOpen = true;
			return;
		}
		const result = await addActiveIndicatorToChart(0);
		if (result.ok) toast.success(`${activeItem?.title ?? 'Indicator'} added to the chart.`);
		else toast.error(result.message ?? 'Could not add this indicator to the chart.');
	}

	async function handlePickPane(paneIndex: number) {
		panePickerOpen = false;
		const result = await addActiveIndicatorToChart(paneIndex);
		if (result.ok) toast.success(`${activeItem?.title ?? 'Indicator'} added to pane ${paneIndex + 1}.`);
		else toast.error(result.message ?? 'Could not add this indicator to the chart.');
	}

	async function handleDelete() {
		if (!activeItem) return;
		await deleteIndicator(activeItem.id);
		deleteConfirmOpen = false;
	}
</script>

<Resizable.PaneGroup direction="vertical" autoSaveId="senken.charts.indicatorDockHeight" class="min-h-0 flex-1">
	<Resizable.Pane defaultSize={70} minSize={30}>
		<div class="relative flex h-full min-h-0 flex-1">
			{@render children()}
		</div>
	</Resizable.Pane>
	<Resizable.Handle withHandle />
	<!-- `open` flows one way, out to this pane (the `$effect` above) — it is
	     never fed back from `onCollapse`/`onExpand`. paneforge reports a
	     freshly registered, still-at-`defaultSize={0}` pane as "collapsed"
	     the moment it mounts, before this component's own effect has had a
	     chance to expand it; wiring those callbacks back into `open` would
	     have that spurious first "collapsed" event stomp a remembered
	     `open: true` back to `false` on every page load. -->
	<Resizable.Pane
		bind:this={dockPane}
		defaultSize={0}
		minSize={18}
		maxSize={65}
		collapsible
		collapsedSize={0}
	>
		<div data-indicator-dock class="flex h-full min-h-0 flex-col bg-chrome">
			<IndicatorToolbar
				active={activeItem}
				dirty={indicatorEditor.dirty}
				saving={indicatorEditor.saving}
				diagnosticsCount={indicatorEditor.diagnostics.length}
				toolchain={indicatorEditor.toolchain}
				{hasEverCompiled}
				onNew={handleNew}
				onSave={() => void saveIndicator()}
				onAddToChart={() => void handleAddToChart()}
				onRename={() => (renameDialogOpen = true)}
				onDelete={() => (deleteConfirmOpen = true)}
				onClose={() => toggleDock(false)}
			/>
			<div class="flex min-h-0 flex-1">
				<IndicatorList
					items={indicatorEditor.list}
					activeId={indicatorEditor.activeId}
					loading={indicatorEditor.listLoading}
					onSelect={(id) => requestSelect(id)}
					onNew={handleNew}
				/>
				<div class="flex min-h-0 flex-1 flex-col p-2.5">
					{#if !indicatorEditor.activeId}
						<div class="flex min-h-0 flex-1 flex-col items-center justify-center gap-2 text-center">
							<span class="font-mono text-[10px] tracking-[0.06em] text-dim2">No indicator open.</span>
							<button
								type="button"
								class="cursor-pointer border border-ink/14 px-2.5 py-1 font-mono text-[9px] tracking-[0.14em] text-foreground hover:bg-ink/7"
								onclick={handleNew}
							>
								NEW INDICATOR
							</button>
						</div>
					{:else}
						<IndicatorSourceEditor
							source={indicatorEditor.draft.source}
							diagnostics={indicatorEditor.diagnostics}
							onSourceChange={setDraftSource}
							onSave={() => void saveIndicator()}
						/>
					{/if}
				</div>
			</div>
		</div>
	</Resizable.Pane>
</Resizable.PaneGroup>

<NewIndicatorDialog
	open={newDialogOpen}
	creating={creatingNew}
	onCreate={(title, template) => void handleCreate(title, template)}
	onOpenChange={(v) => (newDialogOpen = v)}
/>

<RenameIndicatorDialog
	open={renameDialogOpen}
	currentTitle={indicatorEditor.draft.title}
	saving={indicatorEditor.saving}
	onRename={(title) => {
		if (indicatorEditor.activeId) void renameIndicator(indicatorEditor.activeId, title);
		renameDialogOpen = false;
	}}
	onOpenChange={(v) => (renameDialogOpen = v)}
/>

<ConfirmDialog
	open={deleteConfirmOpen}
	title="Delete indicator?"
	description={`Delete "${activeItem?.title ?? ''}"? This removes it from every chart it is placed on.`}
	confirmLabel="Delete"
	onConfirm={() => void handleDelete()}
	onOpenChange={(v) => (deleteConfirmOpen = v)}
/>

<ConfirmDialog
	open={indicatorEditor.pendingSelectId !== undefined}
	title="Discard changes?"
	description="This indicator has unsaved changes. Switching now discards them."
	confirmLabel="Discard"
	onConfirm={confirmDiscardAndSelect}
	onOpenChange={(v) => {
		if (!v) cancelDiscard();
	}}
/>

<PanePickerDialog
	open={panePickerOpen}
	{paneCount}
	onPick={(paneIndex) => void handlePickPane(paneIndex)}
	onOpenChange={(v) => (panePickerOpen = v)}
/>
