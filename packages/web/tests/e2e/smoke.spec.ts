// Three end-to-end checks against a real `senken serve`, never a mock. The
// first two prove the harness itself works end to end; the third is a
// required-red test, alongside `chart-pane.browser-test.ts` — see
// `AGENTS.md`'s "prove the property, do not assert it": a chart page nobody
// scrolls should not keep hitting the network, and today it does (63 calls
// in 20 s against the same bug this suite exercises live).
import { test as base, expect } from '@playwright/test';
import { test, assertEnvironmentIsAlive } from './fixtures';

base('logs in through the form and lands on the dashboard', async ({ page }) => {
	await page.goto('/login');
	await assertEnvironmentIsAlive(page);

	await page.getByLabel('EMAIL').fill('admin@mail.com');
	await page.getByLabel('PASSWORD').fill('e2e-pass-123456');
	await page.getByLabel('PASSWORD').press('Enter');

	await page.waitForURL('**/dashboard');
	const token = await page.evaluate(() => localStorage.getItem('senken.credential.embedded'));
	expect(token).toBeTruthy();
});

test('opening and closing the workspace menu leaves the page clickable', async ({ page }) => {
	await assertEnvironmentIsAlive(page);

	const trigger = page.locator('[aria-haspopup="menu"]').first();
	await trigger.click();
	await expect(page.locator('[role="menu"]')).toBeVisible();

	await page.keyboard.press('Escape');

	// This is the exact reading three QA sessions on 2026-09-03 mis-reported
	// as "body locked forever": true only when the pane never paints, and
	// this suite's own `assertEnvironmentIsAlive` above already refused that
	// environment before this test could run at all.
	await expect
		.poll(() => page.evaluate(() => getComputedStyle(document.body).pointerEvents))
		.not.toBe('none');
	const centerIsOverlay = await page.evaluate(() => {
		const el = document.elementFromPoint(720, 450);
		return el?.hasAttribute('data-dialog-overlay') || el?.hasAttribute('data-overlay') || false;
	});
	expect(centerIsOverlay).toBe(false);
});

test('a chart left alone stops fetching', async ({ page }) => {
	await assertEnvironmentIsAlive(page);

	const barRequests: string[] = [];
	page.on('request', (request) => {
		if (request.url().includes('/api/bars/')) barRequests.push(request.url());
	});

	await page.goto('/charts');
	await page.waitForTimeout(5000);
	const afterFirstWindow = barRequests.length;

	await page.waitForTimeout(5000);
	const afterSecondWindow = barRequests.length;

	// Red today: the bar-loading effect in `chart-pane.svelte` re-triggers
	// itself (`AGENTS.md`'s "derive from what you hold, not from what you
	// opened with"). 032 owns the fix; this stays red until it lands.
	expect(afterSecondWindow - afterFirstWindow).toBeLessThanOrEqual(2);
});
