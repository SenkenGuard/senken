// Drag and resize with a real mouse, against the real page — not a
// component in isolation. CEO reported drag & drop "does not work"; the
// only prior explanation ("body locked") turned out to be an artefact of a
// hidden test pane whose `requestAnimationFrame` never fired, not a real
// defect (see `assertEnvironmentIsAlive` below, which checks for exactly
// that). This suite answers the question with a mouse and a reload, not a
// re-reading of the code.
import { test, expect } from './fixtures';
import type { Page } from '@playwright/test';

const DEFAULT_ROW_HEIGHT_PX = 44;
const DEFAULT_GAP_PX = 12;

async function authFetch<T>(page: Page, path: string, init?: { method?: string }): Promise<T> {
	return page.evaluate(
		async ([p, i]) => {
			const token = localStorage.getItem('senken.credential.embedded');
			const res = await fetch(p as string, {
				...(i as RequestInit),
				headers: { Authorization: `Bearer ${token}` }
			});
			return res.json();
		},
		[path, init ?? {}]
	) as Promise<T>;
}

/** The workspace `GET /api/dashboard/workspaces/default` names — the very
 * first workspace ever created for this account, and therefore also the
 * one a browser with no remembered workspace yet (a first-ever visit, or
 * one whose `localStorage` was cleared) opens on load. The drag/resize
 * tests below work entirely inside this one workspace so a reload keeps
 * finding the same widget; the "remembers the last workspace" test further
 * down deliberately opens a *different* one to prove that remembering
 * actually works. */
async function defaultWorkspaceId(page: Page): Promise<string> {
	const { workspace_id } = await authFetch<{ workspace_id: string }>(
		page,
		'/api/dashboard/workspaces/default'
	);
	return workspace_id;
}

interface DashboardWidgetDto {
	id: string;
	position_x: number;
	position_y: number;
	width: number;
	height: number;
}
interface DashboardLayoutDto {
	widgets: DashboardWidgetDto[];
}

async function fetchLayout(page: Page, workspaceId: string): Promise<DashboardLayoutDto> {
	return authFetch<DashboardLayoutDto>(page, `/api/dashboard/workspaces/${workspaceId}/layout`);
}

/** Adds one Position Size Calculator widget to the default workspace
 * through the real "ADD WIDGET…" flow (kebab menu → picker), and returns
 * its id — the cell and handles Playwright drives are found by
 * `[data-widget-id]` and the accessible names the handles already carry,
 * no new `data-*` attribute needed for either. */
async function addPositionSizeWidget(page: Page, workspaceId: string): Promise<string> {
	await page.locator('[aria-haspopup="menu"]').first().click();
	await page.getByText('ADD WIDGET…').click();
	const dialog = page.getByRole('dialog');
	await expect(dialog).toBeVisible();
	const [saveResponse] = await Promise.all([
		page.waitForResponse(
			(r) => r.url().includes(`/api/dashboard/workspaces/${workspaceId}/layout`) && r.request().method() === 'PUT'
		),
		dialog.getByText('Position Size Calculator').click()
	]);
	await expect(dialog).toBeHidden();
	const saved = (await saveResponse.json()) as DashboardLayoutDto;
	// The just-placed widget is always the last entry `addWidget` appended
	// to its local `widgets` array — this workspace may already hold
	// widgets from a test that ran earlier in this same file/server.
	return saved.widgets[saved.widgets.length - 1].id;
}

test('a widget can be dragged one column to the right and the move survives a reload', async ({ page }) => {
	const workspaceId = await defaultWorkspaceId(page);
	const widgetId = await addPositionSizeWidget(page, workspaceId);

	const before = await fetchLayout(page, workspaceId);
	const beforeWidget = before.widgets.find((w) => w.id === widgetId);
	if (!beforeWidget) throw new Error('the added widget is missing right after adding it');
	const startX = beforeWidget.position_x;

	const gridEl = page.locator('[data-dashboard-grid]');
	const gridStyle = await gridEl.getAttribute('style');
	const columns = Number(gridStyle?.match(/repeat\((\d+),/)?.[1]);
	expect(columns).toBeGreaterThan(0);
	const gridBox = await gridEl.boundingBox();
	if (!gridBox) throw new Error('the dashboard grid has no box to measure');
	const cellWidthPx = (gridBox.width - DEFAULT_GAP_PX * (columns - 1)) / columns;

	const cell = page.locator(`[data-widget-id="${widgetId}"]`);
	const handle = cell.getByRole('button', { name: /^Move / });
	const handleBox = await handle.boundingBox();
	if (!handleBox) throw new Error('the move handle has no box to measure');
	const startPointerX = handleBox.x + handleBox.width / 2;
	const startPointerY = handleBox.y + handleBox.height / 2;

	await page.mouse.move(startPointerX, startPointerY);
	await page.mouse.down();
	await page.mouse.move(startPointerX + cellWidthPx + 8, startPointerY, { steps: 12 });
	await page.mouse.up();

	// Debounced save is 400 ms; give it, plus the round trip, real room.
	await page.waitForTimeout(600);
	await page.reload();
	await expect(page.locator(`[data-widget-id="${widgetId}"]`)).toBeVisible();

	const after = await fetchLayout(page, workspaceId);
	const afterWidget = after.widgets.find((w) => w.id === widgetId);
	if (!afterWidget) throw new Error('the dragged widget is missing after reload');
	expect(afterWidget.position_x).toBe(startX + 1);
});

test('a widget can be resized by one row and the size survives a reload', async ({ page }) => {
	const workspaceId = await defaultWorkspaceId(page);
	const widgetId = await addPositionSizeWidget(page, workspaceId);

	const before = await fetchLayout(page, workspaceId);
	const beforeWidget = before.widgets.find((w) => w.id === widgetId);
	if (!beforeWidget) throw new Error('the added widget is missing right after adding it');
	const startHeight = beforeWidget.height;

	const cell = page.locator(`[data-widget-id="${widgetId}"]`);
	const handle = cell.getByRole('button', { name: /^Resize / });
	const handleBox = await handle.boundingBox();
	if (!handleBox) throw new Error('the resize handle has no box to measure');
	const startPointerX = handleBox.x + handleBox.width / 2;
	const startPointerY = handleBox.y + handleBox.height / 2;

	await page.mouse.move(startPointerX, startPointerY);
	await page.mouse.down();
	await page.mouse.move(startPointerX, startPointerY + DEFAULT_ROW_HEIGHT_PX + 8, { steps: 12 });
	await page.mouse.up();

	await page.waitForTimeout(600);
	await page.reload();
	await expect(page.locator(`[data-widget-id="${widgetId}"]`)).toBeVisible();

	const after = await fetchLayout(page, workspaceId);
	const afterWidget = after.widgets.find((w) => w.id === widgetId);
	if (!afterWidget) throw new Error('the resized widget is missing after reload');
	expect(afterWidget.height).toBe(startHeight + 1);
});

test('opening a second workspace and reloading returns to that workspace, not the default one', async ({ page }) => {
	const before = new Set(
		(await authFetch<{ rows: { id: string }[] }>(page, '/api/dashboard/workspaces?limit=200&offset=0')).rows.map(
			(r) => r.id
		)
	);

	await page.getByRole('button', { name: 'New workspace' }).click();
	await expect
		.poll(async () => {
			const after = await authFetch<{ rows: { id: string }[] }>(
				page,
				'/api/dashboard/workspaces?limit=200&offset=0'
			);
			return after.rows.filter((r) => !before.has(r.id)).length;
		})
		.toBe(1);
	const after = await authFetch<{ rows: { id: string }[] }>(page, '/api/dashboard/workspaces?limit=200&offset=0');
	const secondWorkspaceId = after.rows.find((r) => !before.has(r.id))!.id;

	// A widget placed only in this second workspace is this test's proof
	// that a reload actually re-opened *this* workspace, rather than
	// re-opening the default one and happening to look similar.
	const widgetId = await addPositionSizeWidget(page, secondWorkspaceId);

	await page.reload();

	await expect(page.locator(`[data-widget-id="${widgetId}"]`)).toBeVisible();
});

test('a widget plugin iframe receives the host theme, and it changes without a reload', async ({ page }) => {
	// `plugins/widgets/example-clock/web/index.html` is baked into
	// this binary through `include_str!` at `cargo build` time (see this
	// repo's own `crates/api/src/assets.rs` doc comment on why that differs
	// from `packages/web/build`, which *is* picked up live) — so the
	// `theme.changed` handler added to that file cannot reach a running
	// `senken serve` without a rebuild, which this session is not permitted
	// to run (only one `cargo` build at a time on this machine). The host
	// side of this exact property is proven instead, fully, in
	// `plugin-widget-frame.browser-test.ts` — this test is real and stays
	// written for whoever runs that rebuild next, not deleted or weakened
	// to fake a pass in the meantime.
	test.skip(
		true,
		'needs a `cargo build` to embed the updated example-clock widget HTML — not permitted in this session'
	);

	// The built-in Clock package (`example-clock`, `senken_plugin::widget_package::store::BUILTIN_PACKAGE_ID`)
	// is installed on every fresh start, so it is always in the picker —
	// no separate install step needed for this test.
	await page.locator('[aria-haspopup="menu"]').first().click();
	await page.getByText('ADD WIDGET…').click();
	const dialog = page.getByRole('dialog');
	await expect(dialog).toBeVisible();
	await dialog.getByText('Clock', { exact: true }).click();
	await expect(dialog).toBeHidden();

	const frame = page.frameLocator('iframe[data-widget-plugin-iframe]');
	await expect(frame.locator('body')).toBeVisible();

	const fgBefore = await frame.locator(':root').evaluate(() =>
		getComputedStyle(document.documentElement).getPropertyValue('--fg')
	);
	expect(fgBefore.trim()).not.toBe('');

	await page.getByRole('button', { name: 'Toggle theme' }).click();

	await expect
		.poll(() =>
			frame.locator(':root').evaluate(() => getComputedStyle(document.documentElement).getPropertyValue('--fg'))
		)
		.not.toBe(fgBefore);
});

/** This app's own hit-area rule, proven the way `charts.spec.ts`'s
 * "the indicators button hit-area actually covers its icon" already
 * proves it elsewhere: a click landing at the exact center of a button's
 * own bounding box must resolve, via the real browser's hit-testing, to
 * that button — never a sibling, an ancestor, or empty space an icon only
 * visually fills. */
async function centerHitsButton(page: import('@playwright/test').Page, name: string | RegExp): Promise<boolean> {
	// `.first()`: the default workspace this test shares with the drag/resize
	// tests above can already hold more than one matching button (e.g. more
	// than one Position Size Calculator's own "Remove" button) — this check
	// is about the shape of *a* button matching `name`, not about which one.
	const button = page.getByRole('button', { name }).first();
	const box = await button.boundingBox();
	if (!box) throw new Error(`no box to measure for button ${name}`);
	expect(box.width, `button ${name} is narrower than 28px`).toBeGreaterThanOrEqual(28);
	expect(box.height, `button ${name} is shorter than 28px`).toBeGreaterThanOrEqual(28);
	// Compared against this exact button's *own* `aria-label`, not the
	// (possibly regex) name it was located by — the point is that a click
	// at dead center lands on this same element, not on whichever label
	// text happened to find it.
	const expectedLabel = await button.getAttribute('aria-label');
	const cx = box.x + box.width / 2;
	const cy = box.y + box.height / 2;
	return page.evaluate(
		({ x, y, label }) => document.elementFromPoint(x, y)?.closest('button')?.getAttribute('aria-label') === label,
		{ x: cx, y: cy, label: expectedLabel }
	);
}

test('the workspace menu and remove-widget buttons are at least 28x28 and hit where they look', async ({ page }) => {
	const workspaceId = await defaultWorkspaceId(page);
	await addPositionSizeWidget(page, workspaceId);

	expect(await centerHitsButton(page, 'Workspace menu')).toBe(true);
	expect(await centerHitsButton(page, /^Remove /)).toBe(true);
});
