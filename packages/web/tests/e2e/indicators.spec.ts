// The CEO's own description of what this had to become: a Pine-Editor-style
// dock, not a modal buried behind a picker tab — New, write it in Rust,
// Save (compiling automatically), Add to chart, and see a plot. Every step
// here drives the real `/charts` page against a real `senken serve`, never
// a mocked `apiClient`.
import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';

async function authFetch<T>(page: Page, path: string, init?: { method?: string; body?: unknown }): Promise<T> {
	return page.evaluate(
		async ([p, i]) => {
			const token = localStorage.getItem('senken.credential.embedded');
			const res = await fetch(p as string, {
				...(i as RequestInit),
				headers: { Authorization: `Bearer ${token}`, 'Content-Type': 'application/json' }
			});
			return res.json();
		},
		[path, init]
	);
}

/** `GET /api/my/indicators/toolchain` — every test in this file needs a
 * real `cargo build` to succeed, and this machine is the only place that
 * can be confirmed rather than assumed: never write a toolchain fact from
 * memory. A missing toolchain skips with a printed
 * reason, which is not the same thing as a pass. */
async function requireToolchain(page: Page): Promise<void> {
	const status = await authFetch<{ available: boolean; reason?: string | null }>(page, '/api/my/indicators/toolchain');
	test.skip(!status.available, `Rust toolchain not available on this machine: ${status.reason ?? 'unknown reason'}`);
}

function countLayerSettingsButtons(page: Page) {
	return page.locator('button[aria-label$="settings"]').count();
}

/** The pane's settings-chip count right after navigation is not
 * trustworthy on its own read — a chip renders once its own instrument (or,
 * for a layer, its indicator) resolves, which lands a beat after
 * `[data-chart-pane]` itself, so a single immediate read can catch it
 * mid-render. Waiting for two consecutive reads, 200ms apart, to agree is
 * what actually establishes "the layout has finished loading" rather than
 * guessing a fixed delay. */
async function stableLayerSettingsCount(page: Page): Promise<number> {
	let previous = -1;
	for (;;) {
		const current = await countLayerSettingsButtons(page);
		if (current === previous && current > 0) return current;
		previous = current;
		await page.waitForTimeout(200);
	}
}

test.describe('indicator dock', () => {
	// A cold cache build has measured at ~10s on this machine and up to
	// 150s is the contract the compile endpoint documents
	// (`crates/indicator-compile`'s own deadline) — this suite's own
	// timeout has to be wider than that ceiling, not just the warm case.
	test.setTimeout(180_000);

	test('writing an SMA in Rust, saving, and adding it to the chart shows a new plot', async ({ page }) => {
		await requireToolchain(page);

		await page.goto('/charts');
		await expect(page.locator('[data-chart-pane]').first()).toBeVisible();
		const layersBefore = await stableLayerSettingsCount(page);

		await page.keyboard.press('Alt+i');
		await expect(page.getByRole('button', { name: 'NEW', exact: true })).toBeVisible();

		await page.getByRole('button', { name: 'New indicator', exact: true }).click();
		const newDialog = page.getByRole('dialog', { name: 'New indicator' });
		await expect(newDialog).toBeVisible();
		await newDialog.getByLabel('NAME').fill('My SMA');
		await newDialog.getByRole('button', { name: 'SMA EXAMPLE' }).click();

		const firstBuildStart = Date.now();
		await newDialog.getByRole('button', { name: 'Create' }).click();
		// The dialog only closes once `POST /api/my/indicators`'s own compile
		// attempt has returned (create-then-compile is one round trip — see
		// `crates/indicator-compile`) — up to the same 170s ceiling as the
		// "Compiled" wait below, not Playwright's 5s default.
		await expect(newDialog).toBeHidden({ timeout: 170_000 });

		// "First build warms the cache — this can take a minute" while it is
		// still running, then "Compiled" once the component exists.
		await expect(page.getByText('Compiled', { exact: true })).toBeVisible({ timeout: 170_000 });
		const firstBuildMs = Date.now() - firstBuildStart;
		// eslint-disable-next-line no-console -- this duration needs to be recorded, and this is the only place it is observed.
		console.log(`[indicators.spec] first build took ${firstBuildMs} ms`);

		// The editor shows a real focus ring — checked via `getComputedStyle`
		// against the actual built app (Tailwind's generated utilities do not
		// exist in an isolated component mount, only here), not by eye.
		const sourceInput = page.getByTestId('indicator-source-input');
		await sourceInput.focus();
		await expect
			.poll(() => sourceInput.evaluate((el) => getComputedStyle(el).boxShadow))
			.not.toBe('none');

		// A second build (Save again with no real change) proves the cache
		// warms — recorded the same way.
		const secondBuildStart = Date.now();
		await page.getByTestId('indicator-source-input').click();
		await page.keyboard.press('End');
		await page.keyboard.type('\n');
		const saveButton = page.getByRole('button', { name: 'SAVE', exact: true });
		await saveButton.click();
		await expect(page.getByText('Compiled', { exact: true })).toBeVisible({ timeout: 170_000 });
		const secondBuildMs = Date.now() - secondBuildStart;
		console.log(`[indicators.spec] second build took ${secondBuildMs} ms`);

		const computeRequest = page.waitForRequest(
			(req) => req.url().includes('/api/indicators/compute') && req.method() === 'POST' && req.postData()?.includes('my-sma') === true,
			{ timeout: 15_000 }
		);
		await page.getByRole('button', { name: 'ADD TO CHART', exact: true }).click();
		await computeRequest;

		await expect
			.poll(() => countLayerSettingsButtons(page), { timeout: 15_000 })
			.toBe(layersBefore + 1);

		// Without reloading: the picker's own "MY INDICATORS" group has to
		// pick up the indicator this test just saved twice, which depends on
		// `saveIndicator` refreshing this page's indicator catalogue
		// (`+page.svelte`'s own effect on `indicatorEditor.list`) — a reload
		// would refetch everything fresh regardless and prove nothing about
		// that refresh actually firing.
		await page.getByRole('button', { name: 'INDICATORS & LAYERS' }).click();
		await expect(page.getByRole('dialog')).toBeVisible();
		await page.keyboard.type('My SMA');
		await expect(page.getByRole('option').first()).toContainText('MINE');
		await page.keyboard.press('Escape');
		await expect(page.getByRole('dialog')).toBeHidden();
	});

	test('a broken edit does not remove the plot that was already on the chart', async ({ page }) => {
		await requireToolchain(page);

		await page.goto('/charts');
		await expect(page.locator('[data-chart-pane]').first()).toBeVisible();
		// This test depends on the previous one's layer actually being on
		// the chart already — waiting for its own settings chip (rather than
		// "any settings button > 0", which the main-instrument chip alone
		// would already satisfy) establishes that precondition instead of
		// racing the layout's own load.
		// The layer chip's own action buttons are only shown on hover
		// (`pane-header.svelte`'s `group-hover/chip:block`) — a
		// `display:none` element is excluded from the accessibility tree
		// entirely, so `getByRole` can never find it; the plain attribute
		// selector `countLayerSettingsButtons` already uses is what actually
		// sees it, matched here by name too.
		await expect(page.locator('button[aria-label="my/my-sma settings"]')).toBeAttached();
		const layersBefore = await stableLayerSettingsCount(page);

		await page.keyboard.press('Alt+i');
		await page.getByText('My SMA', { exact: true }).click();
		await expect.poll(() => page.getByTestId('indicator-source-input').inputValue()).not.toBe('');

		await page.getByTestId('indicator-source-input').fill('this is not valid rust\n');
		await page.getByRole('button', { name: 'SAVE', exact: true }).click();

		await expect(page.getByText(/Build failed/)).toBeVisible({ timeout: 170_000 });
		// The plot this test opened with is untouched — a failed compile
		// never touches the chart, only the dock's own diagnostics.
		expect(await countLayerSettingsButtons(page)).toBe(layersBefore);
	});

	test('the picker lists my indicators and Enter adds one', async ({ page }) => {
		await requireToolchain(page);

		await page.goto('/charts');
		await expect(page.locator('[data-chart-pane]').first()).toBeVisible();
		// Same precondition as the previous test: the layout this test opens
		// must already show the two earlier tests' own layer before this one
		// measures its "before" count.
		// The layer chip's own action buttons are only shown on hover
		// (`pane-header.svelte`'s `group-hover/chip:block`) — a
		// `display:none` element is excluded from the accessibility tree
		// entirely, so `getByRole` can never find it; the plain attribute
		// selector `countLayerSettingsButtons` already uses is what actually
		// sees it, matched here by name too.
		await expect(page.locator('button[aria-label="my/my-sma settings"]')).toBeAttached();

		await page.getByRole('button', { name: 'INDICATORS & LAYERS' }).click();
		await expect(page.getByRole('dialog')).toBeVisible();
		await page.keyboard.type('My SMA');

		const options = page.getByRole('option');
		await expect(options).toHaveCount(1);
		await expect(options.first()).toHaveAttribute('aria-selected', 'true');
		await expect(options.first()).toContainText('MINE');

		// Enter picks the highlighted row exactly like a click would — proven
		// here by the resulting compute request, the same deterministic
		// signal `test('writing an SMA…')` uses above, rather than a layer
		// count that two earlier tests have already made ambiguous (this
		// workspace now carries more than one `my/my-sma` layer).
		const computeRequest = page.waitForRequest(
			(req) => req.url().includes('/api/indicators/compute') && req.method() === 'POST' && req.postData()?.includes('my-sma') === true,
			{ timeout: 15_000 }
		);
		await page.keyboard.press('Enter');
		await expect(page.getByRole('dialog')).toBeHidden();
		await computeRequest;
	});

	test('the dock icon buttons have a real, clickable hit-area', async ({ page }) => {
		await page.goto('/charts');
		await expect(page.locator('[data-chart-pane]').first()).toBeVisible();
		await page.keyboard.press('Alt+i');

		for (const name of ['New indicator', 'Rename indicator', 'Delete indicator', 'Close indicator editor']) {
			const button = page.getByRole('button', { name, exact: true });
			const box = await button.boundingBox();
			if (!box) throw new Error(`"${name}" has no box to measure`);
			expect(box.width, `${name} width`).toBeGreaterThanOrEqual(28);
			expect(box.height, `${name} height`).toBeGreaterThanOrEqual(28);
			// A click at the icon's own center must land on this button, not
			// merely be contained by it visually.
			const cx = box.x + box.width / 2;
			const cy = box.y + box.height / 2;
			const hitLabel = await page.evaluate(
				([x, y]) => document.elementFromPoint(x, y)?.closest('button')?.getAttribute('aria-label'),
				[cx, cy]
			);
			expect(hitLabel).toBe(name);
		}
	});
});
