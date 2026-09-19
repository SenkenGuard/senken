// Proves the unified Plugins list (`GET /api/plugins`) actually renders,
// filters, and drives the enable/disable switch through the real
// component — not a description of what it should do. A real browser is
// required for the same reason every other interactive test in this
// project needs one: `Switch`'s focus/animation lifecycle depends on a live
// `requestAnimationFrame`, which a hidden pane's harness does not provide.
//
// The two legacy sections (indicator plugins, widget plugins) fetch on
// mount too, so every stub below answers all three surfaces rather than
// letting an unhandled fetch reject and turn into a misleading "load
// failed" state for the one section under test.
import { afterEach, expect, test, vi } from 'vitest';
import { render } from 'vitest-browser-svelte';
import { page } from 'vitest/browser';
import PluginsSection from './plugins-section.svelte';
import type { PluginDto } from '$lib/api/types';

function jsonResponse(body: unknown): Response {
	return new Response(JSON.stringify(body), {
		status: 200,
		headers: { 'Content-Type': 'application/json' }
	});
}

function okxPlugin(overrides: Partial<PluginDto> = {}): PluginDto {
	return {
		id: 'okx',
		name: 'OKX',
		version: '0.0.5',
		kind: 'static',
		contributes: ['venue'],
		state: { state: 'active' },
		enabled: true,
		needs_restart: false,
		...overrides
	};
}

function clockPackage(): PluginDto {
	return {
		id: 'example-clock',
		name: 'Example Clock Widget',
		version: '1.0.0',
		kind: 'package',
		contributes: ['dashboard_widget'],
		state: { state: 'active' },
		enabled: true,
		needs_restart: false
	};
}

// `simulator` in real life: a static plugin whose only contribution is a
// trade adapter, which `crates/api/src/plugin_handlers.rs::to_dto` never
// turns off live (an adapter already holds credentials and may be managing
// open positions) — unlike `okx` above, which is fully gateable.
function tradeOnlyPlugin(overrides: Partial<PluginDto> = {}): PluginDto {
	return {
		id: 'simulator',
		name: 'Simulator',
		version: '0.0.5',
		kind: 'static',
		contributes: ['trade_adapter'],
		state: { state: 'active' },
		enabled: true,
		needs_restart: false,
		...overrides
	};
}

function errorResponse(status: number, message: string): Response {
	return new Response(JSON.stringify({ error: message }), {
		status,
		headers: { 'Content-Type': 'application/json' }
	});
}

function widgetPackage(overrides: Partial<{
	id: string;
	name: string;
	is_builtin: boolean;
}> = {}): { id: string; name: string; version: string; description: string; enabled: boolean; status: { state: string }; digest: string; widget_count: number; is_builtin: boolean } {
	return {
		id: 'example-clock',
		name: 'Example Clock Widget',
		version: '1.0.0',
		description: 'Ships with Senken.',
		enabled: true,
		status: { state: 'active' },
		digest: 'deadbeef',
		widget_count: 1,
		is_builtin: true,
		...overrides
	};
}

function stubFetch(
	plugins: PluginDto[],
	options: { installError?: string; widgetPackages?: ReturnType<typeof widgetPackage>[] } = {}
): ReturnType<typeof vi.fn> {
	const fetchMock = vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
		const url = typeof input === 'string' ? input : input.toString();
		if (url.endsWith('/api/indicators/plugins')) return jsonResponse([]);
		if (url.endsWith('/api/widget-plugins')) {
			return jsonResponse({ packages: options.widgetPackages ?? [] });
		}
		if (url.endsWith('/api/plugins/refresh') && init?.method === 'POST') {
			return jsonResponse({ plugins });
		}
		if (url.endsWith('/api/plugins') && init?.method === 'POST') {
			if (options.installError) return errorResponse(400, options.installError);
			return new Response(JSON.stringify({ id: 'new-plugin' }), {
				status: 201,
				headers: { 'Content-Type': 'application/json' }
			});
		}
		if (url.endsWith('/api/plugins')) return jsonResponse({ plugins });
		if (url.includes('/api/plugins/') && url.endsWith('/enabled') && init?.method === 'POST') {
			const id = decodeURIComponent(url.split('/api/plugins/')[1]!.split('/')[0]!);
			const body = JSON.parse(init.body as string) as { enabled: boolean };
			const plugin = plugins.find((p) => p.id === id);
			if (!plugin) throw new Error(`unknown plugin id in test stub: ${id}`);
			// Mirrors `crates/api/src/plugin_handlers.rs::to_dto`: a static
			// plugin that activated applies its toggle live for everything
			// gateable, so its own `state` flips in step with `enabled` and
			// no restart is needed. A plugin holding a trade adapter is the
			// one exception — that capability is never turned off live, so
			// it keeps reporting itself active and pending a restart with a
			// reason, whatever the stored flag now says.
			const hasTradeAdapter = plugin.contributes.includes('trade_adapter');
			const needsRestart = plugin.kind === 'static' && hasTradeAdapter && !body.enabled;
			const updated: PluginDto = {
				...plugin,
				enabled: body.enabled,
				state:
					plugin.kind === 'static' && !hasTradeAdapter
						? { state: body.enabled ? 'active' : 'disabled' }
						: plugin.state,
				needs_restart: needsRestart,
				restart_reason: needsRestart
					? 'Trading through this plugin stays active until Senken restarts.'
					: null
			};
			return jsonResponse(updated);
		}
		if (url.includes('/api/plugins/') && init?.method === 'DELETE') {
			return new Response(null, { status: 204 });
		}
		throw new Error(`unexpected fetch in a plugins-section test: ${url}`);
	});
	vi.stubGlobal('fetch', fetchMock);
	return fetchMock;
}

afterEach(() => {
	vi.unstubAllGlobals();
});

test('the unified list shows every plugin with its contribution and origin badges', async () => {
	stubFetch([okxPlugin(), clockPackage()]);

	await render(PluginsSection, {});

	await expect
		.element(page.getByTestId('unified-plugin-row-okx'))
		.toBeInTheDocument();
	await expect
		.element(page.getByTestId('unified-plugin-row-example-clock'))
		.toBeInTheDocument();
	await expect
		.element(page.getByTestId('unified-plugin-contributes-okx-venue'))
		.toHaveTextContent('Venue');
	await expect
		.element(page.getByTestId('unified-plugin-origin-okx'))
		.toHaveTextContent('Built-in');
	await expect.element(page.getByTestId('unified-plugins-count')).toHaveTextContent('2 of 2 shown');
});

test('the venue filter narrows the list to venue-contributing plugins only', async () => {
	stubFetch([okxPlugin(), clockPackage()]);

	await render(PluginsSection, {});
	await expect.element(page.getByTestId('unified-plugin-row-okx')).toBeInTheDocument();

	await page.getByTestId('unified-plugins-filter-venue').click();

	await expect.element(page.getByTestId('unified-plugin-row-okx')).toBeInTheDocument();
	await expect.element(page.getByTestId('unified-plugin-row-example-clock')).not.toBeInTheDocument();
	await expect.element(page.getByTestId('unified-plugins-count')).toHaveTextContent('1 of 2 shown');
});

test('typing a search term narrows the list by name', async () => {
	stubFetch([okxPlugin(), clockPackage()]);

	await render(PluginsSection, {});
	await expect.element(page.getByTestId('unified-plugin-row-okx')).toBeInTheDocument();

	await page.getByTestId('unified-plugins-search').fill('clock');

	await expect.element(page.getByTestId('unified-plugin-row-example-clock')).toBeInTheDocument();
	await expect.element(page.getByTestId('unified-plugin-row-okx')).not.toBeInTheDocument();
});

test('toggling a fully-gateable static plugin off calls setEnabled and never shows a restart pill, since it applies live', async () => {
	const fetchMock = stubFetch([okxPlugin()]);

	await render(PluginsSection, {});
	await expect.element(page.getByTestId('unified-plugin-row-okx')).toBeInTheDocument();

	await page.getByTestId('unified-plugin-toggle-okx').click();

	await expect
		.element(page.getByTestId('unified-plugin-needs-restart-okx'))
		.not.toBeInTheDocument();
	await expect.element(page.getByTestId('unified-plugins-restart-banner')).not.toBeInTheDocument();
	await expect.element(page.getByTestId('unified-plugin-state-okx')).toHaveTextContent('Disabled');
	// The call actually happened, with the right id and the right method —
	// not merely that the row updated for some other reason.
	const enabledCalls = fetchMock.mock.calls.filter(([input]) => {
		const url = typeof input === 'string' ? input : (input as URL | Request).toString();
		return url.includes('/api/plugins/okx/enabled');
	});
	expect(enabledCalls.length).toBe(1);
	const [, init] = enabledCalls[0]!;
	expect((init as RequestInit).method).toBe('POST');
	expect(JSON.parse((init as RequestInit).body as string)).toEqual({ enabled: false });
});

test('toggling off a plugin whose only contribution is a trade adapter shows the restart pill with the actual reason, not a blind "restart to apply"', async () => {
	stubFetch([tradeOnlyPlugin()]);

	await render(PluginsSection, {});
	await expect.element(page.getByTestId('unified-plugin-row-simulator')).toBeInTheDocument();

	await page.getByTestId('unified-plugin-toggle-simulator').click();

	await expect
		.element(page.getByTestId('unified-plugin-needs-restart-simulator'))
		.toHaveTextContent('Trading through this plugin stays active until Senken restarts.');
	await expect.element(page.getByTestId('unified-plugins-restart-banner')).toBeInTheDocument();
	// Trading is still live, so the row must keep saying so rather than
	// claiming the plugin is off.
	await expect
		.element(page.getByTestId('unified-plugin-state-simulator'))
		.toHaveTextContent('Active');
});

test('a package toggle never claims a restart is needed, since it takes effect live', async () => {
	stubFetch([clockPackage()]);

	await render(PluginsSection, {});
	await expect.element(page.getByTestId('unified-plugin-row-example-clock')).toBeInTheDocument();

	await page.getByTestId('unified-plugin-toggle-example-clock').click();

	await expect
		.element(page.getByTestId('unified-plugin-needs-restart-example-clock'))
		.not.toBeInTheDocument();
	await expect.element(page.getByTestId('unified-plugins-restart-banner')).not.toBeInTheDocument();
});

test('an empty catalog shows a message and no bare empty area', async () => {
	stubFetch([]);

	await render(PluginsSection, {});

	await expect.element(page.getByTestId('unified-plugins-list')).not.toBeInTheDocument();
	await expect.element(page.getByText('No plugin is registered on this server yet.')).toBeInTheDocument();
});

test('uninstall is disabled with a tooltip for a built-in static plugin', async () => {
	stubFetch([okxPlugin()]);

	await render(PluginsSection, {});
	await expect.element(page.getByTestId('unified-plugin-row-okx')).toBeInTheDocument();

	const uninstall = page.getByTestId('unified-plugin-uninstall-okx');
	await expect.element(uninstall).toHaveAttribute('aria-disabled', 'true');
	await expect
		.element(uninstall)
		.toHaveAttribute('title', 'Built-in plugins can be disabled but not removed.');
});

test('uninstall is disabled with a tooltip for the widget package this server always ships', async () => {
	stubFetch([clockPackage()], { widgetPackages: [widgetPackage({ is_builtin: true })] });

	await render(PluginsSection, {});
	await expect.element(page.getByTestId('unified-plugin-row-example-clock')).toBeInTheDocument();

	const uninstall = page.getByTestId('unified-plugin-uninstall-example-clock');
	await expect.element(uninstall).toHaveAttribute('aria-disabled', 'true');
	await expect
		.element(uninstall)
		.toHaveAttribute('title', 'This plugin ships with Senken and cannot be removed.');
});

test('uninstalling a removable package asks for confirmation, names it, and removes it on confirm', async () => {
	stubFetch([clockPackage()], {
		widgetPackages: [widgetPackage({ id: 'example-clock', is_builtin: false })]
	});

	await render(PluginsSection, {});
	await expect.element(page.getByTestId('unified-plugin-row-example-clock')).toBeInTheDocument();

	await page.getByTestId('unified-plugin-uninstall-example-clock').click();

	await expect
		.element(page.getByText('Uninstall Example Clock Widget? Market data already downloaded is kept.'))
		.toBeInTheDocument();

	const fetchMock = stubFetch([], { widgetPackages: [] });
	await page.getByRole('button', { name: 'Uninstall', exact: true }).click();

	await expect
		.element(page.getByTestId('unified-plugin-row-example-clock'))
		.not.toBeInTheDocument();
	const deleteCalls = fetchMock.mock.calls.filter(([input], index) => {
		const url = typeof input === 'string' ? input : (input as URL | Request).toString();
		const init = fetchMock.mock.calls[index]?.[1] as RequestInit | undefined;
		return url.endsWith('/api/plugins/example-clock') && init?.method === 'DELETE';
	});
	expect(deleteCalls.length).toBe(1);
});

test('an install error is shown in product copy, not a raw status code', async () => {
	stubFetch([okxPlugin()], { installError: 'That does not look like a plugin package or component.' });

	await render(PluginsSection, {});
	await expect.element(page.getByTestId('unified-plugin-row-okx')).toBeInTheDocument();

	const input = page.getByTestId('unified-plugins-file-input').element() as HTMLInputElement;
	const file = new File([new Uint8Array([0x50, 0x4b, 0x03, 0x04])], 'bad.zip', {
		type: 'application/zip'
	});
	const transfer = new DataTransfer();
	transfer.items.add(file);
	input.files = transfer.files;
	input.dispatchEvent(new Event('change', { bubbles: true }));

	await expect
		.element(page.getByTestId('unified-plugins-install-error'))
		.toHaveTextContent('That does not look like a plugin package or component.');
});
