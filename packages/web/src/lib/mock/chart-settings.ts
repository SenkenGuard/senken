// A pane's chart settings and the rows the settings dialog builds from them.
//
// Every field here is applied by something: `chart-pane.svelte` reads most
// of them onto the chart, `pane-header.svelte` reads the status-line ones.
// That is the entry requirement for the type, not a coincidence — a
// settings row that writes a field nothing reads is a control that lies
// about what it does, and eight of them (a logo toggle, buy/sell buttons,
// a currency unit, price-to-bar ratio lock, non-overlapping labels, a plus
// button, and navigation/pane button visibility) were removed rather than
// left switchable: none had any mechanism behind it, and several never
// could here — the chart library draws no navigation buttons and has no
// bar-ratio lock to hold.

import type { Component } from 'svelte';
import ChartBarIcon from '@lucide/svelte/icons/chart-bar';
import ListIcon from '@lucide/svelte/icons/list';
import RulerIcon from '@lucide/svelte/icons/ruler';
import PencilIcon from '@lucide/svelte/icons/pencil';

export type LineStyle = 'SOLID' | 'DASHED' | 'DOTTED';
export const LINE_STYLES: LineStyle[] = ['SOLID', 'DASHED', 'DOTTED'];

export interface ChartSettings {
	bodyUp: string;
	bodyDown: string;
	borderUp: string;
	borderDown: string;
	wickUp: string;
	wickDown: string;
	borders: boolean;
	wicks: boolean;
	colorPrevClose: boolean;
	precision: 'DEFAULT' | '2' | '4' | '5';
	statusSymbol: boolean;
	statusOhlc: boolean;
	statusChange: boolean;
	statusVolume: boolean;
	statusValues: boolean;
	statusInputs: boolean;
	placement: 'RIGHT' | 'LEFT' | 'BOTH';
	countdown: boolean;
	symbolLabel: 'VALUE + LINE' | 'VALUE' | 'HIDDEN';
	highLow: boolean;
	bidAsk: boolean;
	dayOfWeek: boolean;
	timeFormat: '24H' | '12H';
	bgMode: 'THEME' | 'SOLID';
	bgColor: string;
	gridV: boolean;
	gridVColor: string;
	gridH: boolean;
	gridHColor: string;
	crosshair: LineStyle;
	crosshairColor: string;
	watermark: 'HIDDEN' | 'VISIBLE';
	watermarkColor: string;
	scaleTextColor: string;
	scaleFont: number;
	scaleLineColor: string;
	timeAxis: boolean;
	/** How the price axis maps price to pixels. `REGULAR` is linear;
	 * `LOGARITHMIC` suits a long range; `PERCENT` and `INDEXED` are the
	 * relative views. */
	priceScaleMode: 'REGULAR' | 'LOGARITHMIC' | 'PERCENT' | 'INDEXED';
	/** Price increasing downward — the inverted axis some desks read. */
	invertScale: boolean;
	/** Whether the axis refits itself to the data. Dragging the axis turns
	 * this off, which is why it is a setting and not a transient. */
	autoScale: boolean;
	/** How the pane is split between the main chart and its sub-panes, as
	 * the percentages the resizable group reports. Empty means "never
	 * adjusted", and the defaults apply. */
	paneSplit: number[];
	marginTop: number;
	marginBottom: number;
	marginRight: number;
}

/** reference: the `chartSettings` initial state, lines 1514-1528. */
export function defaultChartSettings(): ChartSettings {
	return {
		bodyUp: 'auto', bodyDown: 'auto', borderUp: 'auto', borderDown: 'auto', wickUp: 'auto', wickDown: 'auto',
		borders: true, wicks: true, colorPrevClose: false, precision: 'DEFAULT',
		statusSymbol: true, statusOhlc: true, statusChange: true, statusVolume: false,
		statusValues: true, statusInputs: true,
		placement: 'RIGHT',
		countdown: true, symbolLabel: 'VALUE + LINE', highLow: false, bidAsk: false, dayOfWeek: true, timeFormat: '24H',
		bgMode: 'THEME', bgColor: '#0a0a0c', gridV: true, gridVColor: 'auto', gridH: true, gridHColor: 'auto',
		crosshair: 'DASHED', crosshairColor: 'auto', watermark: 'HIDDEN', watermarkColor: 'auto',
		priceScaleMode: 'REGULAR', invertScale: false, autoScale: true, paneSplit: [],
		scaleTextColor: 'auto', scaleFont: 9, scaleLineColor: 'auto', timeAxis: true,
		marginTop: 10, marginBottom: 8, marginRight: 6,
	};
}

/** reference: `chartTheme()`, line 1917 — the theme-dependent fallbacks a
 * color field shows while its setting is `'auto'`. Only the fields the
 * settings rows actually use are kept. */
export interface ChartThemeDefaults {
	bg: string;
	axis: string;
	grid: string;
	border: string;
	cross: string;
	up: string;
	wickUp: string;
}

export function chartThemeDefaults(light: boolean): ChartThemeDefaults {
	return light
		? {
				bg: '#f8f8f5',
				axis: 'rgba(18,19,21,0.5)',
				grid: 'rgba(18,19,21,0.06)',
				border: 'rgba(18,19,21,0.12)',
				cross: 'rgba(18,19,21,0.4)',
				up: '#17181a',
				wickUp: 'rgba(18,19,21,0.72)'
			}
		: {
				bg: '#0a0a0c',
				axis: 'rgba(255,255,255,0.42)',
				grid: 'rgba(255,255,255,0.035)',
				border: 'rgba(255,255,255,0.09)',
				cross: 'rgba(255,255,255,0.35)',
				up: '#f2f2ef',
				wickUp: 'rgba(255,255,255,0.7)'
			};
}

export type CsSectionKey = 'symbol' | 'status' | 'scales' | 'canvas';

export const CS_SECTIONS: { key: CsSectionKey; label: string; icon: Component }[] = [
	{ key: 'symbol', label: 'SYMBOL', icon: ChartBarIcon },
	{ key: 'status', label: 'STATUS LINE', icon: ListIcon },
	{ key: 'scales', label: 'SCALES & LINES', icon: RulerIcon },
	{ key: 'canvas', label: 'CANVAS', icon: PencilIcon }
];

/** One rendered row of the settings body (reference: the `csRows` row
 * shapes built by `chk`/`colorRow`/`optRow`/`numRow`/`grp`, lines 2614-2637).
 * A row can combine a checkbox with a color field (e.g. "BORDERS · UP"), so
 * these are flags rather than a discriminated union, matching the reference. */
export interface SettingsRow {
	label: string;
	isGroup?: boolean;
	hasCheck?: boolean;
	checked?: boolean;
	onToggle?: () => void;
	hasColor?: boolean;
	colorValue?: string;
	colorFallback?: string;
	onColorChange?: (hex: string) => void;
	hasOptions?: boolean;
	options?: { label: string; active: boolean; onPick: () => void }[];
	hasNumber?: boolean;
	value?: string;
	unit?: string;
	onInc?: () => void;
	onDec?: () => void;
	/** A capability-gated control remains visible to explain why it cannot be
	 * used, rather than appearing to accept a setting it cannot honour. */
	disabled?: boolean;
	disabledReason?: string;
}

/** Builds the rows for one section (reference: the `if (s.csSection === …)`
 * chain, lines 2639-2711). `set` patches the settings object in place —
 * callers own the actual `$state` and pass a setter, mirroring the
 * reference's `this.setCS(patch)`. */
export function buildSettingsRows(
	section: CsSectionKey,
	cs: ChartSettings,
	set: (patch: Partial<ChartSettings>) => void,
	theme: ChartThemeDefaults,
	capabilities: { quotes?: boolean } = {}
): { groupLabel: string; rows: SettingsRow[] } {
	const chk = (label: string, key: keyof ChartSettings): SettingsRow => ({
		label,
		hasCheck: true,
		checked: cs[key] !== false,
		onToggle: () => set({ [key]: cs[key] === false } as Partial<ChartSettings>)
	});
	const colorRow = (label: string, key: keyof ChartSettings, fallback: string): SettingsRow => ({
		label,
		hasColor: true,
		colorValue: cs[key] as string,
		colorFallback: fallback,
		onColorChange: (hex) => set({ [key]: hex } as Partial<ChartSettings>)
	});
	const optRow = (label: string, key: keyof ChartSettings, opts: string[]): SettingsRow => ({
		label,
		hasOptions: true,
		options: opts.map((o) => ({ label: o, active: cs[key] === o, onPick: () => set({ [key]: o } as Partial<ChartSettings>) }))
	});
	const numRow = (label: string, key: keyof ChartSettings, unit: string, min: number, max: number, step = 1): SettingsRow => ({
		label,
		hasNumber: true,
		value: String(cs[key]),
		unit,
		onInc: () => set({ [key]: Math.min(max, (cs[key] as number) + step) } as Partial<ChartSettings>),
		onDec: () => set({ [key]: Math.max(min, (cs[key] as number) - step) } as Partial<ChartSettings>)
	});
	const group = (label: string): SettingsRow => ({ label, isGroup: true });

	if (section === 'symbol') {
		return {
			groupLabel: 'CANDLES',
			rows: [
				{
					label: 'COLOR BARS BASED ON PREVIOUS CLOSE',
					hasCheck: true,
					checked: cs.colorPrevClose,
					onToggle: () => set({ colorPrevClose: !cs.colorPrevClose })
				},
				colorRow('BODY · UP', 'bodyUp', theme.up),
				colorRow('BODY · DOWN', 'bodyDown', '#3a3b3d'),
				{ ...chk('BORDERS · UP', 'borders'), hasColor: true, colorValue: cs.borderUp, colorFallback: theme.up, onColorChange: (hex) => set({ borderUp: hex }) },
				{ label: 'BORDERS · DOWN', hasColor: true, colorValue: cs.borderDown, colorFallback: '#6a6b6d', onColorChange: (hex) => set({ borderDown: hex }) },
				{ ...chk('WICK · UP', 'wicks'), hasColor: true, colorValue: cs.wickUp, colorFallback: theme.wickUp, onColorChange: (hex) => set({ wickUp: hex }) },
				{ label: 'WICK · DOWN', hasColor: true, colorValue: cs.wickDown, colorFallback: '#6a6b6d', onColorChange: (hex) => set({ wickDown: hex }) },
				group('DATA MODIFICATION'),
				optRow('PRECISION', 'precision', ['DEFAULT', '2', '4', '5'])
			]
		};
	}
	if (section === 'status') {
		return {
			groupLabel: 'INSTRUMENT',
			rows: [
				chk('TITLE & TIMEFRAME', 'statusSymbol'),
				chk('CHART VALUES (OHLC)', 'statusOhlc'),
				chk('BAR CHANGE VALUES', 'statusChange'),
				chk('VOLUME', 'statusVolume'),
				group('INDICATORS'),
				chk('TITLES', 'statusValues'),
				chk('INPUTS IN TITLE', 'statusInputs')
			]
		};
	}
	if (section === 'scales') {
		return {
			groupLabel: 'PRICE SCALE',
			rows: [
				optRow('SCALES PLACEMENT', 'placement', ['RIGHT', 'LEFT', 'BOTH']),
				group('PRICE LABELS & LINES'),
				chk('COUNTDOWN TO BAR CLOSE', 'countdown'),
				optRow('SYMBOL LABEL', 'symbolLabel', ['VALUE + LINE', 'VALUE', 'HIDDEN']),
				chk('HIGH AND LOW LINES', 'highLow'),
				{
					...chk('BID AND ASK LINES', 'bidAsk'),
					disabled: capabilities.quotes === false,
					disabledReason:
						capabilities.quotes === false
							? 'This source does not report best bid and ask prices.'
							: undefined
				},
				group('TIME SCALE'),
				chk('TIME AXIS', 'timeAxis'),
				chk('DAY OF WEEK ON LABELS', 'dayOfWeek'),
				optRow('TIME FORMAT', 'timeFormat', ['24H', '12H'])
			]
		};
	}
	if (section === 'canvas') {
		return {
			groupLabel: 'CHART BASIC STYLES',
			rows: [
				optRow('BACKGROUND', 'bgMode', ['THEME', 'SOLID']),
				{
					label: 'BACKGROUND COLOR',
					hasColor: true,
					colorValue: cs.bgMode === 'SOLID' ? cs.bgColor : 'auto',
					colorFallback: theme.bg,
					onColorChange: (hex) => set({ bgColor: hex, bgMode: 'SOLID' })
				},
				{ ...chk('VERTICAL GRID LINES', 'gridV'), hasColor: true, colorValue: cs.gridVColor, colorFallback: theme.grid, onColorChange: (hex) => set({ gridVColor: hex }) },
				{ ...chk('HORIZONTAL GRID LINES', 'gridH'), hasColor: true, colorValue: cs.gridHColor, colorFallback: theme.grid, onColorChange: (hex) => set({ gridHColor: hex }) },
				{ label: 'CROSSHAIR COLOR', hasColor: true, colorValue: cs.crosshairColor, colorFallback: theme.cross, onColorChange: (hex) => set({ crosshairColor: hex }) },
				optRow('CROSSHAIR STYLE', 'crosshair', LINE_STYLES),
				optRow('WATERMARK', 'watermark', ['HIDDEN', 'VISIBLE']),
				{ label: 'WATERMARK COLOR', hasColor: true, colorValue: cs.watermarkColor, colorFallback: 'rgba(255,255,255,0.06)', onColorChange: (hex) => set({ watermarkColor: hex }) },
				group('SCALES'),
				{ label: 'TEXT COLOR', hasColor: true, colorValue: cs.scaleTextColor, colorFallback: theme.axis, onColorChange: (hex) => set({ scaleTextColor: hex }) },
				numRow('TEXT SIZE', 'scaleFont', 'PX', 8, 16, 1),
				{ label: 'SCALE LINES', hasColor: true, colorValue: cs.scaleLineColor, colorFallback: theme.border, onColorChange: (hex) => set({ scaleLineColor: hex }) },
				group('MARGINS'),
				numRow('TOP', 'marginTop', '%', 0, 40, 1),
				numRow('BOTTOM', 'marginBottom', '%', 0, 40, 1),
				numRow('RIGHT', 'marginRight', 'BARS', 0, 40, 1)
			]
		};
	}
	return { groupLabel: '', rows: [] };
}
