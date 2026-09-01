// Global command-palette overlay state.
//
// The reference's command palette is
// one piece of chrome shared by every page through the single-file
// component's own `this.state.cmd` — it is what "ADD WIDGET…" (dashboard),
// "ATTACH ACCOUNT" / the account chip (trade engine) and the symbol readout
// / "INDICATORS & LAYERS" (charts) all open, each in a different `cmd` mode
// (line 2716: `symbol` | `account` | `adapter` | `widget` | `layer`).
//
// its architectural decision (see the implementation report): this is a
// module-level rune store, not Svelte context. `layout/command-palette.svelte`
// is mounted once, in `AppShell`, so it is already inside every route's
// component tree — a context would have to be set at that same single
// mount point anyway, buying nothing over a plain exported store, while a
// module singleton lets any leaf button (a dashboard widget-menu item, an
// engine adapter card, a charts-page toolbar button) open the palette by
// importing this file directly, with no prop-drilling through the route
// tree and no risk of "used outside provider" if a future route renders
// before the shell does. Senken is a single-window Tauri app, not a
// multi-tenant SSR server, so the usual reason to prefer context (isolating
// state per request) does not apply here.
import type { Component } from 'svelte';
import type { Tone } from '$lib/mock/engine';

export type CommandMode = 'symbol' | 'account' | 'adapter' | 'widget' | 'layer';

export interface CommandRow {
	icon: Component;
	title: string;
	sub: string;
	meta: string;
	metaTone: Tone;
	onPick: () => void;
	/** Shown, but not selectable. A row that cannot do what picking it
	 * promises belongs in the list — leaving it out would read as "no such
	 * instrument" — but choosing it must not strand the caller. `meta`
	 * carries the reason. */
	disabled?: boolean;
}

export interface CommandKindTab {
	label: string;
	/** A getter, not a plain boolean: `kindTabs` is built once, when the
	 * request opens (`openCommand`), but which tab is active can keep
	 * changing while the palette stays open (the layer picker's own
	 * INSTRUMENT/INDICATOR/STRATEGY state). A plain boolean captured at
	 * build time would go stale the moment the caller's state changes;
	 * calling this each render keeps it live. */
	active: () => boolean;
	onClick: () => void;
}

export interface CommandRequest {
	mode: CommandMode;
	placeholder: string;
	footer: string;
	/** Only the 'layer' mode has these (reference: `addKinds`, line 3279) —
	 * the INSTRUMENT / INDICATOR / STRATEGY strip above the row list. */
	kindTabs?: CommandKindTab[];
	rows: (query: string) => CommandRow[];
	/** Whether the rows are still being fetched. A getter for the same reason
	 * `CommandKindTab.active` is one: the request is built once when the
	 * palette opens, while this keeps changing underneath it. Without it an
	 * in-flight search renders as "NO MATCH", which is not merely unhelpful —
	 * it states something untrue about the venue's catalogue. */
	busy?: () => boolean;
}

class CommandPaletteStore {
	open = $state(false);
	query = $state('');
	request = $state<CommandRequest | null>(null);
}

export const commandPalette = new CommandPaletteStore();

/** reference: the `openAccountCmd` / `openAdapterCmd` / the widget-menu's
 * `onClick`, and the charts toolbar's symbol/layer triggers — all of them
 * are `this.setState({ cmd: <mode>, cmdQuery: '' })`. */
export function openCommand(request: CommandRequest) {
	commandPalette.request = request;
	commandPalette.query = '';
	commandPalette.open = true;
}

/** reference: `closeCmd`, line 3230, and the global Escape handler
 * (line 1588) that also clears `cmd`/`cmdQuery`. */
export function closeCommand() {
	commandPalette.open = false;
	commandPalette.query = '';
	commandPalette.request = null;
}
