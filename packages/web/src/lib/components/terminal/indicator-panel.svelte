<script lang="ts">
	// Indicator-authoring panel: write indicator-lang, compile it, place the
	// result on the chart — steps 1-3 of the plan this panel implements.
	// Reachable from `routes/charts/+page.svelte` the same way
	// `layer-dialog.svelte` already is (that dialog's own doc: "reachable
	// only from the charts route"), since `chart-pane.svelte` itself has no
	// notion of its own pane index or of `addIndicatorLayer` — only the
	// route does.
	//
	// "Pasang ke chart memakai jalur yang sama dengan indikator bawaan":
	// placing a compiled indicator calls the exact same
	// `addIndicatorLayer`/`removeLayer` the built-in picker in
	// `+page.svelte` already calls, with the exact same `IndicatorCatalogItem`
	// shape (`catalogItemFromEntry`, `$lib/components/terminal/indicator-
	// panel.ts`) — never a second placement path, and never a new write
	// against the chart itself, so every existing `holdViewport` guard in
	// `chart-pane.svelte` already covers it.
	//
	// Editing a number and re-running must update the existing plot, not
	// grow a second one beside it (see `indicator-panel.ts`'s
	// `applyRecompile` for the property and its own test for the proof):
	// this component tracks the id of the layer *it* last placed and
	// removes that one first, so a recompile nets to the same one layer.
	//
	// The compiler runs in-process on the server, in microseconds — no
	// spinner here on purpose; a `disabled` button state during the
	// in-flight request is enough.
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import XIcon from '@lucide/svelte/icons/x';
	import IndicatorPanelEditor, { type CompileErrorInfo } from './indicator-panel-editor.svelte';
	import { DEFAULT_INDICATOR_SOURCE, catalogItemFromEntry, findJustPlacedLayer } from './indicator-panel';
	import { apiClient, readCompileIndicatorError } from '$lib/api/client';
	import { HttpError, getErrorMessage } from '$lib/api/errors';
	import { chartWorkspaceStore, addIndicatorLayer, removeLayer } from '$lib/charts/workspace-store.svelte';
	import type { IndicatorCatalogEntry, IndicatorSummaryDto } from '$lib/api/types';

	let {
		paneIndex,
		onClose
	}: {
		/** Which pane a successful compile places the indicator on. `null`
		 * while the panel is closed — `Dialog.Root`'s own `open` is derived
		 * from this, the same way `layer-dialog.svelte`'s is derived from its
		 * `layer` prop being non-null. */
		paneIndex: number | null;
		onClose: () => void;
	} = $props();

	let source = $state(DEFAULT_INDICATOR_SOURCE);
	let compileError = $state<CompileErrorInfo | null>(null);
	/** Set when the source compiled but could not be added to this server's
	 * live indicator catalogue — including the current, honestly-reported
	 * gap where `POST /api/indicators/compile` compiles successfully and
	 * then fails to register (see this panel's own plan doc). Never
	 * papered over as a success. */
	let placeError = $state<string | null>(null);
	let lastEntry = $state<IndicatorCatalogEntry | null>(null);
	/** The layer id *this panel* most recently placed, so the next run
	 * removes it before adding the freshly compiled one instead of piling
	 * up a second plot beside it. */
	let placedLayerId = $state<string | null>(null);
	let running = $state(false);
	let saveName = $state('');
	let saveBusy = $state(false);
	let saved = $state<IndicatorSummaryDto[]>([]);
	/** Set when publishing to, or loading from, the registry fails —
	 * distinct from `compileError`/`placeError`, which are about the
	 * compiler and the chart, not the registry. */
	let registryError = $state<string | null>(null);

	/** Reseeds the draft only when the panel opens *fresh* on a different
	 * pane, never on every re-render — an in-progress edit must survive a
	 * re-render the same way `layer-dialog.svelte`'s own draft does. */
	let openedFor = $state<number | null>(null);
	$effect(() => {
		if (paneIndex === null) {
			openedFor = null;
			return;
		}
		if (openedFor === paneIndex) return;
		openedFor = paneIndex;
		source = DEFAULT_INDICATOR_SOURCE;
		compileError = null;
		placeError = null;
		lastEntry = null;
		placedLayerId = null;
		saveName = '';
		registryError = null;
		void refreshSaved();
	});

	/** `GET /api/registry/indicators/mine` — every indicator this account
	 * has published, regardless of which browser or session published it,
	 * unlike a client-only remembered list. */
	async function refreshSaved() {
		try {
			const page = await apiClient.listMyIndicators(50, 0);
			saved = page.rows;
		} catch (error) {
			registryError = getErrorMessage(error, 'Could not load your saved indicators.');
		}
	}

	async function run() {
		if (paneIndex === null || running) return;
		const target = paneIndex;
		running = true;
		compileError = null;
		placeError = null;
		try {
			const entry = await apiClient.compileIndicator(source);
			lastEntry = entry;
			const item = catalogItemFromEntry(entry);
			if (placedLayerId) await removeLayer(target, placedLayerId);
			await addIndicatorLayer(target, item);
			const layers = chartWorkspaceStore.layout?.panes[target]?.layers ?? [];
			placedLayerId = findJustPlacedLayer(layers, item.name)?.id ?? null;
		} catch (error) {
			if (error instanceof HttpError) {
				const parsed = readCompileIndicatorError(error.body);
				if (parsed) {
					compileError = parsed;
					running = false;
					return;
				}
			}
			// A mistake in the source has its own line/column and is handled
			// above; anything else — including the dynamic-indicator
			// registration step failing past a clean compile — is reported
			// here, in full, rather than silently discarded.
			placeError = getErrorMessage(error, 'Could not add this indicator to the chart.');
		} finally {
			running = false;
		}
	}

	/** `POST /api/registry/indicators` — publishes under the caller's own
	 * namespace, replacing an earlier publish of the same name (see that
	 * endpoint's own doc). */
	async function save() {
		const name = saveName.trim();
		if (!name || saveBusy) return;
		saveBusy = true;
		registryError = null;
		try {
			await apiClient.publishIndicator(name, source);
			await refreshSaved();
		} catch (error) {
			registryError = getErrorMessage(error, 'Could not save this indicator.');
		} finally {
			saveBusy = false;
		}
	}

	/** `GET /api/registry/indicators/{namespace}/{name}` — fetches the full
	 * source (a listing row never carries it) and loads it into the
	 * editor. */
	async function load(entry: IndicatorSummaryDto) {
		registryError = null;
		try {
			const full = await apiClient.getRegistryIndicator(entry.namespace, entry.name);
			source = full.source;
			saveName = full.name;
			compileError = null;
			placeError = null;
		} catch (error) {
			registryError = getErrorMessage(error, 'Could not load this indicator.');
		}
	}
</script>

<Dialog.Root
	open={paneIndex !== null}
	onOpenChange={(v) => {
		if (!v) onClose();
	}}
>
	<Dialog.Content
		showCloseButton={false}
		class="top-[74px] flex max-h-[80%] w-[560px] max-w-[94%] translate-y-0 flex-col gap-0 overflow-hidden border-ink/18 bg-card2 p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		<Dialog.Header class="flex-row items-center justify-between gap-3 space-y-0 px-4 py-[13px]">
			<Dialog.Title class="truncate text-[13px] font-medium tracking-[0.03em] text-foreground">INDICATOR EDITOR</Dialog.Title>
			<Dialog.Close class="flex-none cursor-pointer text-dim2" aria-label="Close">
				<XIcon class="size-[14px]" />
			</Dialog.Close>
		</Dialog.Header>
		<Dialog.Description class="sr-only">Author, compile and place a custom indicator</Dialog.Description>

		<div class="flex min-h-0 flex-1 flex-col gap-3 overflow-auto px-4 py-[14px]">
			<IndicatorPanelEditor {source} error={compileError} onSourceChange={(v) => (source = v)} />

			{#if placeError}
				<div data-place-error class="border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 font-mono text-[10.5px] text-amber-300">
					{placeError}
				</div>
			{/if}

			{#if lastEntry && !compileError && !placeError}
				<div data-compile-success class="font-mono text-[10px] tracking-[0.1em] text-dim2">
					ON CHART: {lastEntry.short_title}
				</div>
			{/if}

			{#if registryError}
				<div data-registry-error class="border border-amber-500/40 bg-amber-500/10 px-2.5 py-2 font-mono text-[10.5px] text-amber-300">
					{registryError}
				</div>
			{/if}

			<div class="flex items-center gap-2">
				<input
					type="text"
					placeholder="Name to save as…"
					class="h-[26px] flex-1 border border-ink/14 bg-transparent px-2.5 text-[11px] text-foreground outline-none focus:border-foreground"
					bind:value={saveName}
				/>
				<button
					type="button"
					class="cursor-pointer border border-ink/14 px-3 py-[6px] disabled:cursor-default disabled:opacity-40"
					onclick={save}
					disabled={!saveName.trim() || saveBusy}
				>
					<span class="font-mono text-[9px] tracking-[0.16em] text-dim2">SAVE</span>
				</button>
			</div>

			{#if saved.length > 0}
				<div data-saved-indicators class="flex flex-col gap-1">
					<span class="font-mono text-[8.5px] tracking-[0.16em] text-dim">MY INDICATORS</span>
					{#each saved as entry (entry.id)}
						<button
							type="button"
							class="cursor-pointer truncate border border-ink/10 px-2 py-1 text-left font-mono text-[10.5px] text-foreground"
							onclick={() => load(entry)}
						>
							{entry.name}
						</button>
					{/each}
				</div>
			{/if}
		</div>

		<div class="flex flex-none items-center justify-end gap-[7px] border-t border-ink/10 px-4 py-[11px]">
			<button type="button" class="cursor-pointer border border-ink/14 px-4 py-[7px]" onclick={onClose}>
				<span class="font-mono text-[9px] tracking-[0.18em] text-dim2">CLOSE</span>
			</button>
			<button
				type="button"
				class="cursor-pointer bg-foreground px-[18px] py-[7px] disabled:cursor-default disabled:opacity-40"
				onclick={run}
				disabled={running}
			>
				<span class="font-mono text-[9px] font-semibold tracking-[0.18em] text-inv">
					{placedLayerId ? 'RUN' : 'RUN & ADD TO CHART'}
				</span>
			</button>
		</div>
	</Dialog.Content>
</Dialog.Root>
