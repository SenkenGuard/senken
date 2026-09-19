<script lang="ts">
	// The workspace's widget grid: native CSS Grid, no layout dependency —
	// `grid-template-columns`/`grid-auto-rows` place every widget from the
	// same integer columns/rows the server stores, so the browser's own
	// grid engine (not this component) recomputes pixel positions whenever
	// the container is resized. Drag and resize are built on plain pointer
	// events (`pointerdown`/`pointermove`/`pointerup` with pointer
	// capture) — no drag-and-drop library.
	//
	// This component owns the imperative, lifecycle-y half of the grid
	// (pointer tracking, debounced saves); the pure math it leans on
	// (overlap/bounds checks, pixel-delta-to-grid-delta) lives in
	// `./geometry.ts`, unit-tested on its own since this component itself
	// cannot be rendered through this project's server-only Svelte test
	// harness (see `widget-frame.svelte`'s own doc comment for why).
	import { onDestroy } from 'svelte';
	import { toast } from 'svelte-sonner';
	import { cn } from '$lib/utils.js';
	import {
		getDashboardLayout,
		replaceDashboardLayout,
		type DashboardLayoutDto,
		type DashboardWidgetDefinition,
		type DashboardWidgetDto,
		type DashboardWidgetPlacementRequest
	} from './api';
	import {
		DEFAULT_GAP_PX,
		DEFAULT_ROW_HEIGHT_PX,
		clampRectToGrid,
		fitsWithinColumns,
		pixelDeltaToGridDelta,
		rectanglesOverlap,
		type GridMetrics,
		type GridRect
	} from './geometry';
	import { rendererFor } from './widget-registry.svelte';
	import WidgetFrame from './widget-frame.svelte';

	let {
		layout,
		catalog,
		onLayoutSaved
	}: {
		layout: DashboardLayoutDto;
		catalog: DashboardWidgetDefinition[];
		/** Called after a save actually lands on the server, with the
		 * layout it returned — the caller uses this only to keep its own
		 * `revision` in sync; it is never the trigger for a re-render (the
		 * grid already updated itself optimistically). */
		onLayoutSaved?: (layout: DashboardLayoutDto) => void;
	} = $props();

	// Local, optimistic copy: the grid moves instantly under the pointer.
	// Once mounted, this component is the source of truth for its own
	// `widgets`/`columns`/`revision` — a save's own response, or an
	// explicit reload after a conflict, is what updates them from here on,
	// never the `layout` prop re-rendering with the same workspace open
	// (which would snap a widget back mid-drag the same way the opening-
	// window bug this project has hit before did). Switching to a
	// *different* workspace is the one case local state should not
	// survive — the caller does that by keying this component on
	// `layout.workspace.id` (see `routes/dashboard/+page.svelte`), which
	// remounts it fresh rather than asking it to notice the prop changed
	// out from under it.
	// `$state.snapshot`, not `structuredClone`: `layout` is a `$state` proxy in
	// the caller, so `layout.widgets` is a proxied array and `structuredClone`
	// throws `DataCloneError` on it. The snapshot is Svelte's own proxy-aware
	// deep copy, which is what "seed once, then own it" needs here.
	let widgets = $state<DashboardWidgetDto[]>($state.snapshot(layout.widgets));
	let columns = $state(layout.workspace.columns);
	let revision = $state(layout.workspace.revision);

	// Ids `addWidget` generated locally for optimism, that the server has
	// never seen: a request naming one with `id: Some(..)` would fail
	// (`senken_dashboard::DashboardError::WidgetNotFound`), since a
	// widget's id is only ever assigned by the server on insert. The
	// request omits `id` for these instead — a genuinely new row — and a
	// successful save's own response (which echoes back the *real* ids)
	// is what clears this set.
	let unconfirmedIds = new Set<string>();

	let gridEl: HTMLDivElement | undefined;
	let saveTimer: ReturnType<typeof setTimeout> | undefined;
	let saving = false;

	function rectOf(widget: DashboardWidgetDto): GridRect {
		return { x: widget.position_x, y: widget.position_y, width: widget.width, height: widget.height };
	}

	function otherRects(exceptId: string): GridRect[] {
		return widgets.filter((w) => w.id !== exceptId).map(rectOf);
	}

	function isValidPlacement(rect: GridRect, exceptId: string): boolean {
		if (!fitsWithinColumns(rect, columns)) return false;
		return !otherRects(exceptId).some((other) => rectanglesOverlap(rect, other));
	}

	function scheduleSave() {
		if (saveTimer) clearTimeout(saveTimer);
		// UI moves locally while a drag is in progress; the save itself is
		// debounced until interaction settles, then sent as one full
		// snapshot — see `senken_dashboard::DashboardWorkspaceStore::replace_layout`'s
		// own docs for why add/move/resize/delete are all just this one
		// call.
		saveTimer = setTimeout(saveNow, 400);
	}

	async function saveNow() {
		if (saving) {
			// A save is already in flight; the trailing edit will be
			// captured by the *next* scheduled save once this one settles
			// (its `.finally` below reschedules if anything changed since).
			scheduleSave();
			return;
		}
		saving = true;
		const body: { expected_revision: number; columns: number; widgets: DashboardWidgetPlacementRequest[] } = {
			expected_revision: revision,
			columns,
			widgets: widgets.map((w) => ({
				// `undefined` (not `w.id`) for a widget this component
				// generated locally and the server has never confirmed —
				// see `unconfirmedIds`'s own doc comment above.
				id: unconfirmedIds.has(w.id) ? undefined : w.id,
				provider_id: w.provider_id,
				widget_type_id: w.widget_type_id,
				position_x: w.position_x,
				position_y: w.position_y,
				width: w.width,
				height: w.height,
				visible: w.visible,
				config: w.config,
				config_schema_version: w.config_schema_version
			}))
		};
		try {
			const result = await replaceDashboardLayout(layout.workspace.id, body);
			// The response is authoritative, ids included — every widget
			// that was pending a server-assigned id now has one.
			widgets = result.widgets;
			revision = result.workspace.revision;
			unconfirmedIds.clear();
			onLayoutSaved?.(result);
		} catch (error) {
			const status = (error as { status?: number }).status;
			if (status === 409) {
				toast.error('This dashboard changed in another tab. Reloading its layout.');
				// The server is the only source of truth once two writers
				// disagree — replace the local snapshot outright rather than
				// retry blindly against a revision that is already stale.
				try {
					const fresh = await getDashboardLayout(layout.workspace.id);
					widgets = fresh.widgets;
					columns = fresh.workspace.columns;
					revision = fresh.workspace.revision;
					onLayoutSaved?.(fresh);
				} catch {
					toast.error('Could not reload the dashboard layout.');
				}
			} else {
				toast.error('Could not save the dashboard layout.');
			}
		} finally {
			saving = false;
		}
	}

	interface DragState {
		id: string;
		startRect: GridRect;
		startPointerX: number;
		startPointerY: number;
		metrics: GridMetrics;
		mode: 'move' | 'resize';
	}
	let drag = $state<DragState | null>(null);
	// The id of the widget currently under a drag whose *proposed* placement
	// overlaps another widget or runs off the grid — set only while that
	// stays true, so the cell can show it was rejected instead of just not
	// moving with no visible reason (`isValidPlacement`'s own check below is
	// unchanged; this only adds feedback around it).
	let invalidPlacementId = $state<string | null>(null);

	// The two classes a drag/resize in progress puts on `document.body`,
	// removed unconditionally on pointer up, pointer cancel, *and* this
	// component's own destruction — the last of those matters because a
	// workspace switch remounts this whole component (`{#key}` in
	// `routes/dashboard/+page.svelte`) mid-drag would otherwise leave a
	// `cursor-grabbing` class on `body` that nothing left mounted would ever
	// clear, which is exactly the shape of stuck-cursor bug this project has
	// already misdiagnosed once as "the body is locked".
	function bodyDragClasses(mode: 'move' | 'resize'): string[] {
		return mode === 'move' ? ['select-none', 'cursor-grabbing'] : ['select-none', 'cursor-nwse-resize'];
	}
	function clearBodyDragClasses(): void {
		document.body.classList.remove('select-none', 'cursor-grabbing', 'cursor-nwse-resize');
	}
	onDestroy(clearBodyDragClasses);

	function beginDrag(id: string, mode: 'move' | 'resize', event: PointerEvent) {
		if (!gridEl) return;
		const widget = widgets.find((w) => w.id === id);
		if (!widget) return;
		const containerWidthPx = gridEl.getBoundingClientRect().width;
		(event.currentTarget as Element).setPointerCapture(event.pointerId);
		drag = {
			id,
			startRect: rectOf(widget),
			startPointerX: event.clientX,
			startPointerY: event.clientY,
			metrics: {
				containerWidthPx,
				columns,
				rowHeightPx: DEFAULT_ROW_HEIGHT_PX,
				gapPx: DEFAULT_GAP_PX
			},
			mode
		};
		invalidPlacementId = null;
		document.body.classList.add(...bodyDragClasses(mode));
	}

	function onPointerMove(event: PointerEvent) {
		if (!drag) return;
		const { dx, dy } = pixelDeltaToGridDelta(
			event.clientX - drag.startPointerX,
			event.clientY - drag.startPointerY,
			drag.metrics
		);
		const proposed: GridRect =
			drag.mode === 'move'
				? { ...drag.startRect, x: drag.startRect.x + dx, y: drag.startRect.y + dy }
				: { ...drag.startRect, width: drag.startRect.width + dx, height: drag.startRect.height + dy };
		const clamped = clampRectToGrid(proposed, columns);
		if (!isValidPlacement(clamped, drag.id)) {
			invalidPlacementId = drag.id;
			return;
		}
		invalidPlacementId = null;
		widgets = widgets.map((w) =>
			w.id === drag!.id
				? { ...w, position_x: clamped.x, position_y: clamped.y, width: clamped.width, height: clamped.height }
				: w
		);
	}

	function onPointerUp() {
		if (!drag) return;
		drag = null;
		invalidPlacementId = null;
		clearBodyDragClasses();
		scheduleSave();
	}

	/** How long a removed widget's "Undo" toast stays actionable before its
	 * removal actually reaches the server — long enough to notice and react
	 * to, short enough that a workspace nobody touches again still settles
	 * quickly. */
	const REMOVE_UNDO_WINDOW_MS = 5000;

	function removeWidget(id: string) {
		const index = widgets.findIndex((w) => w.id === id);
		if (index === -1) return;
		const removed = widgets[index];
		widgets = widgets.filter((w) => w.id !== id);

		// The removal is optimistic in the UI immediately, but `scheduleSave`
		// (and therefore the actual server write) is held off for the whole
		// undo window — "Undo" restores this exact object, `config` and all,
		// to its original index, with nothing ever having been sent.
		let undone = false;
		const commitTimer = setTimeout(() => {
			if (!undone) scheduleSave();
		}, REMOVE_UNDO_WINDOW_MS);

		toast('Widget removed', {
			duration: REMOVE_UNDO_WINDOW_MS,
			action: {
				label: 'Undo',
				onClick: () => {
					if (undone) return;
					undone = true;
					clearTimeout(commitTimer);
					const insertAt = Math.min(index, widgets.length);
					widgets = [...widgets.slice(0, insertAt), removed, ...widgets.slice(insertAt)];
				}
			}
		});
	}

	/** A dynamic widget's own `plugin-widget-frame.svelte` host calls this
	 * after the widget inside its sandboxed iframe successfully patches its
	 * own config through `config.patch` — the same debounced full-snapshot
	 * save every move/resize/add/remove already goes through, so a config
	 * edit is never a second, differently-shaped write path. */
	function updateWidgetConfig(id: string, config: string) {
		widgets = widgets.map((w) => (w.id === id ? { ...w, config } : w));
		scheduleSave();
	}

	function firstFreeRect(width: number, height: number): GridRect {
		// Simple top-left-first placement: scan row by row for the first
		// column offset where a `width`x`height` rectangle fits without
		// overlapping anything already placed.
		const existing = widgets.map(rectOf);
		for (let y = 0; y < 200; y++) {
			for (let x = 0; x + width <= columns; x++) {
				const candidate: GridRect = { x, y, width, height };
				if (!existing.some((other) => rectanglesOverlap(candidate, other))) return candidate;
			}
		}
		return { x: 0, y: 0, width, height };
	}

	/** Every widget type currently placed in this grid — read by the
	 * "ADD WIDGET…" picker to draw its "Added" badge. A plain function, not
	 * `$derived`, because this component's own exported API is otherwise
	 * only functions (`addWidget`); called from inside a caller's own
	 * `$derived`, a read of `widgets` here is still tracked exactly the way
	 * it would be if the caller read `widgets` directly — Svelte's
	 * reactivity graph is not scoped to the component a `$state` happens to
	 * be declared in. */
	export function placedWidgetTypeIds(): Set<string> {
		return new Set(widgets.map((w) => w.widget_type_id));
	}

	/** How many widgets are placed in this grid right now — read by the
	 * "delete workspace" confirmation so it can name the actual count
	 * ("Its 4 widgets will be removed."), not the count `layout.widgets`
	 * held when the page first loaded (this grid's own local `widgets` is
	 * the source of truth once mounted; see this component's own header
	 * comment). */
	export function widgetCount(): number {
		return widgets.length;
	}

	export function addWidget(definition: DashboardWidgetDefinition) {
		const rect = firstFreeRect(definition.default_size.width, definition.default_size.height);
		const tempId = crypto.randomUUID();
		unconfirmedIds.add(tempId);
		widgets = [
			...widgets,
			{
				id: tempId,
				provider_id: definition.provider_id,
				widget_type_id: definition.widget_type_id,
				position_x: rect.x,
				position_y: rect.y,
				width: rect.width,
				height: rect.height,
				visible: true,
				config: '{}',
				config_schema_version: 1,
				created_at: 0,
				updated_at: 0
			}
		];
		scheduleSave();
	}

	const catalogById = $derived(new Map(catalog.map((c) => [c.widget_type_id, c])));
</script>

<div
	bind:this={gridEl}
	class="grid gap-3"
	style={`grid-template-columns: repeat(${columns}, minmax(0, 1fr)); grid-auto-rows: ${DEFAULT_ROW_HEIGHT_PX}px;`}
	data-dashboard-grid
	role="group"
	aria-label="Dashboard widgets"
	onpointermove={onPointerMove}
	onpointerup={onPointerUp}
	onpointercancel={onPointerUp}
>
	{#each widgets as widget (widget.id)}
		{@const renderer = rendererFor(widget, (config) => updateWidgetConfig(widget.id, config))}
		{@const definition = catalogById.get(widget.widget_type_id)}
		{@const isDragging = drag?.id === widget.id}
		{@const isInvalidPlacement = invalidPlacementId === widget.id}
		<div
			style={`grid-column: ${widget.position_x + 1} / span ${widget.width}; grid-row: ${widget.position_y + 1} / span ${widget.height};`}
			data-dashboard-widget-cell
			data-widget-id={widget.id}
			class={cn(
				isDragging && 'opacity-90',
				isDragging && (isInvalidPlacement ? 'ring-1 ring-loss' : 'ring-1 ring-foreground/40')
			)}
		>
			<WidgetFrame
				title={definition?.title ?? widget.widget_type_id}
				dataSource={definition?.data_source}
				isPlaceholder={!renderer}
				onRemove={() => removeWidget(widget.id)}
				onDragHandlePointerDown={(event) => beginDrag(widget.id, 'move', event)}
				onResizeHandlePointerDown={(event) => beginDrag(widget.id, 'resize', event)}
			>
				{#if renderer}
					{@const Renderer = renderer.component}
					<Renderer {...renderer.props()} />
				{/if}
			</WidgetFrame>
		</div>
	{/each}
</div>
