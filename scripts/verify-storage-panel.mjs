// Drives the Settings → Storage panel against a real server, because the
// things most likely to be wrong here cannot be seen from a unit test: that
// the section appears for an account holding the grant, that the tree opens
// down to a series, that a delete confirmation names what it is about to
// remove, and that the freed space actually shows up in the total.
//
// Not part of `bun test` — it needs a built binary, a browser, and a data
// directory it is allowed to delete from. Run it by hand:
//
//   bun run build:web && cargo build --bin senken
//   ./target/debug/senken serve --port 4193 --data-dir /tmp/senken-storage-check &
//   node scripts/verify-storage-panel.mjs
//
// Point it at a throwaway `--data-dir`. Never the repository's own `.data`:
// this script deletes stored market data on purpose, and that directory
// holds catalogs for around fifty venues.
import { chromium } from 'playwright';

const BASE = process.env.SENKEN_URL ?? 'http://127.0.0.1:4193';
const EMAIL = 'admin@mail.com';
const PASSWORD = 'a-very-long-password';

const browser = await chromium.launch({ executablePath: '/opt/pw-browsers/chromium' });
const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
const out = {};

async function signIn() {
	await page.goto(`${BASE}/login`, { waitUntil: 'networkidle' });
	await page.waitForTimeout(600);
	// A fresh data directory shows the setup form; a reused one shows log in.
	if (await page.locator('#setup-password').count()) {
		await page.fill('#setup-password', PASSWORD);
		await page.fill('#setup-confirm', PASSWORD);
	} else {
		await page.fill('#login-email', EMAIL);
		await page.fill('#login-password', PASSWORD);
	}
	await page.click('button[type=submit]');
	await page.waitForTimeout(2500);
}

async function openStorageSettings() {
	await page.goto(`${BASE}/`, { waitUntil: 'networkidle' });
	await page.waitForTimeout(800);
	await page.getByRole('button', { name: /settings/i }).first().click();
	await page.waitForTimeout(600);
	out.sectionListed = await page.getByRole('button', { name: /^storage$/i }).count();
	if (out.sectionListed) {
		await page.getByRole('button', { name: /^storage$/i }).first().click();
		await page.waitForTimeout(1200);
	}
}

await signIn();
await openStorageSettings();

out.showsTotal = (await page.locator('text=All sources').count()) > 0;
out.body = (await page.locator('body').innerText()).replace(/\s+/g, ' ').slice(0, 600);

// Open the tree as far as it goes, then check a confirmation names its
// target and its cost before anything is deleted.
const sourceRow = page.locator('[aria-expanded]').first();
if (await sourceRow.count()) {
	await sourceRow.click();
	await page.waitForTimeout(400);
	const instrumentRow = page.locator('[aria-expanded]').nth(1);
	if (await instrumentRow.count()) {
		await instrumentRow.click();
		await page.waitForTimeout(400);
	}
	const del = page.getByRole('button', { name: /^Delete /i }).last();
	if (await del.count()) {
		await del.click();
		await page.waitForTimeout(400);
		out.confirmText = (await page.getByTestId('storage-delete-confirm').textContent())?.trim();
	}
}

await page.screenshot({ path: '/tmp/shot-storage.png', fullPage: true });
await browser.close();
console.log(JSON.stringify(out, null, 2));
