// A logged-in page, without ever typing into the login form (that path has
// its own test in `smoke.spec.ts`). Mirrors exactly what the client itself
// does on a successful login (`credential-store.ts`'s
// `STORAGE_KEY_PREFIX + serverId`, `servers.svelte.ts`'s embedded server id)
// rather than reimplementing the auth flow a second, divergent way.
import { test as base, expect } from '@playwright/test';
import { readFile } from 'node:fs/promises';
import { join } from 'node:path';

const AUTH_STATE_FILE = join(import.meta.dirname, '.auth.json');

async function readToken(): Promise<string> {
	const { token } = JSON.parse(await readFile(AUTH_STATE_FILE, 'utf8')) as { token: string };
	return token;
}

/** Same probe every browser test session in this project requires: a
 * lifecycle observation (a menu closing, an overlay clearing) means nothing
 * if the page never actually paints. Failing here, loudly, is the point —
 * see `src/test-support/browser-setup.ts`'s sibling in the Vitest harness
 * for the full story of why. */
async function assertEnvironmentIsAlive(page: import('@playwright/test').Page): Promise<void> {
	const visibilityState = await page.evaluate(() => document.visibilityState);
	if (visibilityState !== 'visible') {
		throw new Error(`e2e tests need a visible page, got ${visibilityState}`);
	}
	const fired = await page.evaluate(
		() =>
			new Promise<boolean>((resolve) => {
				const timer = setTimeout(() => resolve(false), 500);
				requestAnimationFrame(() => {
					clearTimeout(timer);
					resolve(true);
				});
			})
	);
	if (!fired) throw new Error('requestAnimationFrame did not fire within 500 ms; refusing to run');
}

export const test = base.extend({
	page: async ({ page, baseURL }, use) => {
		const token = await readToken();
		// A real page load is needed before `localStorage` has an origin to
		// write into — `/login` is as good a first stop as any, and it is
		// also the page a fresh, unauthenticated visitor would land on.
		await page.goto('/login');
		await assertEnvironmentIsAlive(page);
		await page.evaluate((t) => localStorage.setItem('senken.credential.embedded', t), token);
		await page.goto('/dashboard');
		await use(page);
	}
});

export { expect, assertEnvironmentIsAlive };
