// Shared dark/light palette for every lightweight-charts instance on the
// charts page (`chartTheme()`, line 1917). Extracted out
// of chart-pane.svelte so sub-pane-chart.svelte
// can use the exact same colors instead of a second copy drifting from it.

export interface ChartPaneTheme {
	bg: string;
	axis: string;
	grid: string;
	border: string;
	cross: string;
	label: string;
	up: string;
	downFill: string;
	down: string;
	wickUp: string;
	wickDown: string;
	/** Direction colours for the last-price badge and line. The candles
	 * themselves stay monochrome by design, so these are the only place a
	 * pane says up or down in colour. */
	gain: string;
	loss: string;
	/** The last-price badge with no direction to report — a venue this build
	 * cannot stream. Solid, not translucent: the badge sits over the price
	 * ticks, and anything see-through reads as two numbers on top of each
	 * other. */
	neutral: string;
	onNeutral: string;
}

export function chartPaneThemeColors(dark: boolean): ChartPaneTheme {
	return dark
		? {
				bg: '#0a0a0c',
				axis: 'rgba(255,255,255,0.42)',
				grid: 'rgba(255,255,255,0.035)',
				border: 'rgba(255,255,255,0.09)',
				cross: 'rgba(255,255,255,0.35)',
				label: '#1a1a1f',
				up: '#f2f2ef',
				downFill: 'rgba(255,255,255,0.06)',
				down: 'rgba(255,255,255,0.55)',
				wickUp: 'rgba(255,255,255,0.7)',
				wickDown: 'rgba(255,255,255,0.4)',
				gain: '#7de0a3',
				loss: '#e8836f',
				neutral: '#2c2c33',
				onNeutral: 'rgba(255,255,255,0.88)'
			}
		: {
				bg: '#f8f8f5',
				axis: 'rgba(18,19,21,0.5)',
				grid: 'rgba(18,19,21,0.06)',
				border: 'rgba(18,19,21,0.12)',
				cross: 'rgba(18,19,21,0.4)',
				label: '#17181a',
				up: '#17181a',
				downFill: 'rgba(18,19,21,0.05)',
				down: 'rgba(18,19,21,0.55)',
				wickUp: 'rgba(18,19,21,0.72)',
				wickDown: 'rgba(18,19,21,0.42)',
				gain: '#3aa76d',
				loss: '#d0553a',
				neutral: '#d9d9d2',
				onNeutral: '#17181a'
			};
}

export function isDarkTheme(): boolean {
	return document.documentElement.classList.contains('dark');
}
