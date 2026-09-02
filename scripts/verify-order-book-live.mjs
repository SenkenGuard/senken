// Watches the order-book panel refresh itself against a real server and a
// real venue, because the property this change is about cannot be seen from
// a unit test: that a reader who touches nothing gets a newer book than the
// one they opened with, and that there is no longer a control asking them
// to fetch it.
//
// Not part of `bun test` — it needs a built binary, a browser, and one live
// venue. Run it by hand:
//
//   bun run build:web && cargo build --bin senken
//   ./target/debug/senken serve --port 4194 --data-dir /tmp/senken-book-check &
//   node scripts/verify-order-book-live.mjs
//
// It watches for a deliberately short window — long enough to see the
// timestamp move more than once, and no longer. The book endpoint is a
// public OKX one and this is the only way to observe the refresh actually
// happening, but it is still venue traffic: do not leave it running.
import { chromium } from 'playwright';

const BASE = process.env.SENKEN_URL ?? 'http://127.0.0.1:4194';
const EMAIL = 'admin@mail.com';
const PASSWORD = 'a-very-long-password';
/** Three refreshes at the server's own one-second cadence, plus slack for
 * the first venue round-trip. */
const WATCH_MS = 6000;

const browser = await chromium.launch({ executablePath: '/opt/pw-browsers/chromium' });
const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
const out = {};

async function signIn() {
	await page.goto(`${BASE}/login`, { waitUntil: 'domcontentloaded' });
	await page.waitForTimeout(600);
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

await signIn();
await page.goto(`${BASE}/charts`, { waitUntil: 'domcontentloaded' });
await page.waitForTimeout(3000);
out.url = page.url();
out.pageText = (await page.locator('body').innerText()).replace(/\s+/g, ' ').slice(0, 300);

// Open whichever panel holds the book. The charts page opens with every
// panel closed, so this has to find and press its own way in.
const opener = page.getByRole('button', { name: /order book/i }).first();
out.panelOpener = (await opener.count()) > 0;
if (out.panelOpener) {
	await opener.click();
	await page.waitForTimeout(2500);
}

const panel = page.locator('[data-book-state]').first();
await panel.waitFor({ state: 'attached', timeout: 20000 }).catch(() => {});
out.instrument = await page
	.locator('[data-pane-instrument]')
	.first()
	.getAttribute('data-pane-instrument')
	.catch(() => null);
out.state = await panel.getAttribute('data-book-state').catch(() => null);

// The control that must be gone, checked in the DOM rather than by eye —
// Tailwind reports nothing for a class that does not exist, and neither does
// a button that was only visually hidden.
out.refreshControls = await page.getByRole('button', { name: /refresh order book/i }).count();

// The reader touches nothing from here on. Anything that changes, changed
// on its own.
const stamps = new Set();
const ladders = new Set();
const started = Date.now();
while (Date.now() - started < WATCH_MS) {
	const stamp = await panel.locator('[data-book-updated-at]').getAttribute('data-book-updated-at').catch(() => null);
	if (stamp) stamps.add(stamp);
	const ladder = await panel.innerText().catch(() => '');
	if (ladder) ladders.add(ladder.replace(/\s+/g, ' '));
	await page.waitForTimeout(400);
}

out.distinctTimestamps = stamps.size;
out.distinctLadders = ladders.size;
out.refreshedUnprompted = stamps.size > 1;
out.sample = [...stamps].slice(0, 5);
out.finalState = await panel.getAttribute('data-book-state').catch(() => null);

console.log(JSON.stringify(out, null, 2));
await browser.close();
