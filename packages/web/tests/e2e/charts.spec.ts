// Two things QA could not reproduce anywhere but the real `/charts` page —
// Enter in the indicator picker, and the toolbar button's first click — so
// this suite exercises them against the real page rather than a component
// in isolation (`command-palette.browser-test.ts` already proves the
// palette component itself is not at fault: Enter works there).
import { test, expect } from './fixtures';

/** Every layer chip's settings button is labelled `"<layer label> settings"`
 * (`pane-header.svelte`), and the pane's own main-instrument chip carries
 * one too — so the count includes that one fixed chip plus one per real
 * layer. Comparing the count before and after an action proves whether a
 * layer was actually added without having to know the pane's starting
 * layer count. */
function countLayerSettingsButtons(page: import('@playwright/test').Page) {
	return page.locator('button[aria-label$="settings"]').count();
}

test('pressing Enter in the indicator picker adds a layer', async ({ page }) => {
	await page.goto('/charts');
	await expect(page.locator('[data-chart-pane]').first()).toBeVisible();

	const layersBefore = await countLayerSettingsButtons(page);

	await page.getByRole('button', { name: 'INDICATORS & LAYERS' }).click();
	await expect(page.getByRole('dialog')).toBeVisible();
	await page.keyboard.type('SMA');

	const options = page.getByRole('option');
	await expect(options).toHaveCount(1);
	await expect(options.first()).toHaveAttribute('aria-selected', 'true');

	await page.keyboard.press('Enter');

	await expect(page.getByRole('dialog')).toBeHidden();
	// A generous poll window: this is the first interactive test to run
	// against a freshly booted `senken serve`, which is still warming its
	// live-feed venue catalog in the background (visible in its own logs as
	// several seconds of connection warnings) — the layer add's own
	// `replaceLayout` round trip can queue behind that on a slow run.
	await expect
		.poll(() => countLayerSettingsButtons(page), { timeout: 15_000 })
		.toBe(layersBefore + 1);
});

test('the indicators button opens the picker on the first click after load', async ({ page }) => {
	await page.goto('/charts');
	await expect(page.locator('[data-chart-pane]').first()).toBeVisible();

	await page.getByRole('button', { name: 'INDICATORS & LAYERS' }).click();
	await expect(page.getByRole('dialog')).toBeVisible({ timeout: 1000 });
});

test('the indicators button hit-area actually covers its icon', async ({ page }) => {
	await page.goto('/charts');
	await expect(page.locator('[data-chart-pane]').first()).toBeVisible();

	const button = page.getByRole('button', { name: 'INDICATORS & LAYERS' });
	const box = await button.boundingBox();
	if (!box) throw new Error('the indicators button has no box to measure');
	// The center of the button's own box is, by construction, also the
	// center of the icon it contains — this is what QA reported failing:
	// a click landing here doing nothing, on the first try, after a fresh
	// load.
	const cx = box.x + box.width / 2;
	const cy = box.y + box.height / 2;
	const hit = await page.evaluate(
		([x, y]) => document.elementFromPoint(x, y)?.closest('button')?.getAttribute('aria-label'),
		[cx, cy]
	);
	expect(hit).toBe('INDICATORS & LAYERS');
});

test("the indicator picker's search input shows a focus-visible ring", async ({ page }) => {
	await page.goto('/charts');
	await expect(page.locator('[data-chart-pane]').first()).toBeVisible();

	await page.getByRole('button', { name: 'INDICATORS & LAYERS' }).click();
	const input = page.getByPlaceholder('Search instruments or indicators…');
	// `onOpenAutoFocus` focuses it programmatically as the dialog opens. The
	// indicator is a `box-shadow` ring, not `outline` — `outline-none` also
	// sits on this input (to suppress the browser's own default), and
	// Tailwind v4's outline utilities share one `--tw-outline-style` custom
	// property across an element, so a `focus-visible:outline-*` utility
	// here would have inherited that `none` rather than overriding it. Ring
	// is a different property entirely, and is the same idiom this app's
	// own `ui/input` already uses. Checked via `getComputedStyle` against
	// the real, fully-styled page — Tailwind's generated utilities only
	// exist here, not in an isolated component mount (see
	// `command-palette.browser-test.ts`'s own note on why this assertion
	// does not live there).
	await expect(input).toBeFocused();
	const boxShadow = await input.evaluate((el) => getComputedStyle(el).boxShadow);
	expect(boxShadow).not.toBe('none');
});
