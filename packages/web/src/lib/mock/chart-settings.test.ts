import { describe, expect, test } from 'bun:test';
import { CS_SECTIONS, buildSettingsRows, chartThemeDefaults, defaultChartSettings } from './chart-settings';

describe('bid and ask setting capability gate', () => {
	test('a source without quotes disables the control and explains why', () => {
		const { rows } = buildSettingsRows(
			'scales',
			defaultChartSettings(),
			() => {},
			chartThemeDefaults(false),
			{ quotes: false }
		);
		const row = rows.find((candidate) => candidate.label === 'BID AND ASK LINES');

		expect(row?.disabled).toBe(true);
		expect(row?.disabledReason).toBe('This source does not report best bid and ask prices.');
	});

	test('a source that reports quotes leaves the control available', () => {
		const { rows } = buildSettingsRows(
			'scales',
			defaultChartSettings(),
			() => {},
			chartThemeDefaults(false),
			{ quotes: true }
		);
		const row = rows.find((candidate) => candidate.label === 'BID AND ASK LINES');

		expect(row?.disabled).toBe(false);
		expect(row?.disabledReason).toBeUndefined();
	});
});

test('settings expose only controls backed by the chart data surface', () => {
	expect(CS_SECTIONS.map((section) => section.key)).not.toContain('trading');
	const settings = defaultChartSettings();
	expect(settings).not.toHaveProperty('showPositions');
	expect(settings).not.toHaveProperty('showOrders');
	expect(settings).not.toHaveProperty('tif');
});
