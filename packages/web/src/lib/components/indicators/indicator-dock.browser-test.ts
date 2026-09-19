// Interactive proof for the indicator-authoring dock — New → dialog → Save
// → diagnostics → discard-guard → delete-confirm → Cmd+S, none of which the
// server-only Svelte harness can exercise: they need a real focused
// textarea and real keyboard events. `apiClient.myIndicators` is
// monkey-patched per test (a plain object of methods, not a class the
// singleton hides) rather than mocked globally, so each test controls
// exactly what the server would have said without touching a real network.
import { test, expect, vi, beforeEach } from 'vitest';
import { page, userEvent } from 'vitest/browser';
import { render } from 'vitest-browser-svelte';
import IndicatorDock from './indicator-dock.svelte';
import { apiClient } from '$lib/api/client';
import { indicatorEditor } from '$lib/charts/indicator-editor.svelte';
import { createRawSnippet } from 'svelte';
import type {
	IndicatorToolchainStatusResponse,
	SaveUserIndicatorResponse,
	UserIndicatorDto,
	UserIndicatorSummaryDto
} from '$lib/api/types';

const READY_TOOLCHAIN: IndicatorToolchainStatusResponse = { available: true, reason: null };

function summary(over: Partial<UserIndicatorSummaryDto> = {}): UserIndicatorSummaryDto {
	return {
		id: 'ind-1',
		title: 'My SMA',
		slug: 'my-sma',
		compiled: true,
		compile_error: null,
		updated_at: 0,
		...over
	};
}

const emptyChildren = createRawSnippet(() => ({ render: () => '<div></div>' }));

/** `IndicatorDock`'s root is a `paneforge` `PaneGroup`, which sizes itself
 * from its own container's *computed* height — real usage always has one,
 * inherited from `+page.svelte`'s own viewport-height flex chrome, but a
 * component mounted on its own by this harness has no such ancestor, so
 * every pane inside it (and everything in it) would otherwise report a
 * height of zero and never receive a click. Giving the render container an
 * explicit height is the harness-side fix, not a production concern. */
async function renderDock(props: { paneCount: number; children: typeof emptyChildren }) {
	document.documentElement.style.height = '100%';
	document.body.style.height = '100%';
	document.body.style.margin = '0';
	const result = await render(IndicatorDock, props);
	result.baseElement.style.height = '100%';
	result.container.style.height = '640px';
	result.container.style.display = 'flex';
	result.container.style.flexDirection = 'column';
	return result;
}

function mockClient(over: Partial<typeof apiClient.myIndicators> = {}, toolchain: IndicatorToolchainStatusResponse = READY_TOOLCHAIN) {
	apiClient.myIndicators.list = vi.fn(async () => []);
	apiClient.myIndicators.get = vi.fn(async () => {
		throw new Error('not stubbed');
	});
	apiClient.myIndicators.create = vi.fn(async () => {
		throw new Error('not stubbed');
	});
	apiClient.myIndicators.update = vi.fn(async () => {
		throw new Error('not stubbed');
	});
	apiClient.myIndicators.remove = vi.fn(async () => {});
	apiClient.myIndicators.toolchain = vi.fn(async () => toolchain);
	apiClient.listIndicators = vi.fn(async () => []);
	Object.assign(apiClient.myIndicators, over);
}

// Every test starts from a clean, never-opened editor: the store is a
// module-level singleton (same shape as `workspace-store.svelte.ts`), so a
// value left over from a previous test would otherwise leak into the next
// one's assertions.
beforeEach(() => {
	indicatorEditor.open = true;
	indicatorEditor.list = [];
	indicatorEditor.activeId = null;
	indicatorEditor.draft = { title: '', source: '' };
	indicatorEditor.saved = null;
	indicatorEditor.diagnostics = [];
	indicatorEditor.toolchain = null;
	indicatorEditor.pendingSelectId = undefined;
	mockClient();
});

test('creating an indicator through the dialog adds it to the list and opens it in the editor', async () => {
	const created = summary({ id: 'new-1', title: 'My SMA', slug: 'my-sma', compiled: true });
	mockClient({
		create: vi.fn(async (): Promise<SaveUserIndicatorResponse> => ({ id: 'new-1', compiled: true, diagnostics: null })),
		list: vi.fn(async () => [created])
	});

	await renderDock({ paneCount: 1, children: emptyChildren });

	await userEvent.click(page.getByRole('button', { name: 'New indicator', exact: true }));
	const dialog = page.getByRole('dialog', { name: 'New indicator' });
	await expect.element(dialog).toBeVisible();
	await userEvent.fill(dialog.getByLabelText('NAME'), 'My SMA');
	await userEvent.click(dialog.getByRole('button', { name: 'Create' }));

	await expect.element(dialog).not.toBeInTheDocument();
	await expect.element(page.getByText('My SMA')).toBeInTheDocument();
	await expect.poll(() => indicatorEditor.activeId).toBe('new-1');
	await expect.element(page.getByTestId('indicator-source-input')).toBeInTheDocument();
});

test('saving a broken indicator marks the failing line and keeps the previous compiled state', async () => {
	const row = summary({ compiled: true });
	const full: UserIndicatorDto = {
		id: 'ind-1',
		title: 'My SMA',
		slug: 'my-sma',
		source: 'fn broken() {\n  let x = 1\n}\n',
		compiled: true,
		compile_error: null,
		api_version: '0.0.0',
		updated_at: 0
	};
	mockClient({
		list: vi.fn(async () => [row]),
		get: vi.fn(async () => full),
		update: vi.fn(
			async (): Promise<SaveUserIndicatorResponse> => ({
				id: 'ind-1',
				compiled: false,
				diagnostics: [{ line: 7, column: 3, message: 'expected `;`' }]
			})
		)
	});

	const dock = await renderDock({ paneCount: 1, children: emptyChildren });
	await userEvent.click(page.getByText('My SMA'));
	await expect.poll(() => indicatorEditor.activeId).toBe('ind-1');

	const textarea = page.getByTestId('indicator-source-input');
	await userEvent.fill(textarea, 'fn broken() {\n  let x = 1\n  let y = 2\n  let z = 3\n  let w = 4\n  let v = 5\n  bogus\n}\n');
	await userEvent.click(page.getByRole('button', { name: 'SAVE' }));

	await expect.poll(() => dock.container.querySelector('[data-line="7"][data-line-error="true"]')).not.toBeNull();
	await expect.element(page.getByText('Build failed: 1 error')).toBeInTheDocument();
	// The row's own compiled dot still reads "compiled" — the previous
	// component is left in place, per `SaveUserIndicatorResponse`'s own
	// contract, since only `list` (not `update`'s response) drives the
	// list's status dot in this store.
	await expect.poll(() => indicatorEditor.list.find((r) => r.id === 'ind-1')?.compiled).toBe(true);
});

test('switching indicators with unsaved changes asks first', async () => {
	const rowA = summary({ id: 'a', title: 'Indicator A', slug: 'a' });
	const rowB = summary({ id: 'b', title: 'Indicator B', slug: 'b' });
	mockClient({
		list: vi.fn(async () => [rowA, rowB]),
		get: vi.fn(async (id: string) => ({
			id,
			title: id === 'a' ? 'Indicator A' : 'Indicator B',
			slug: id,
			source: `// ${id}\n`,
			compiled: true,
			compile_error: null,
			api_version: '0.0.0',
			updated_at: 0
		}))
	});

	await renderDock({ paneCount: 1, children: emptyChildren });
	await userEvent.click(page.getByText('Indicator A'));
	await expect.poll(() => indicatorEditor.activeId).toBe('a');

	await userEvent.fill(page.getByTestId('indicator-source-input'), '// edited, not saved\n');
	await userEvent.click(page.getByText('Indicator B'));

	const confirmDialog = page.getByRole('alertdialog', { name: 'Discard changes?' });
	await expect.element(confirmDialog).toBeVisible();
	// Escape cancels — the pending switch is abandoned and the editor keeps
	// showing the unsaved draft, not indicator B's source.
	await userEvent.keyboard('{Escape}');
	await expect.element(confirmDialog).not.toBeInTheDocument();
	expect(indicatorEditor.activeId).toBe('a');
	expect(indicatorEditor.draft.source).toBe('// edited, not saved\n');

	await userEvent.click(page.getByText('Indicator B'));
	await expect.element(page.getByRole('alertdialog', { name: 'Discard changes?' })).toBeVisible();
	await userEvent.click(page.getByRole('button', { name: 'Discard' }));
	await expect.poll(() => indicatorEditor.activeId).toBe('b');
});

test('delete asks for confirmation naming the indicator', async () => {
	const row = summary({ title: 'Doomed Indicator' });
	const remove = vi.fn(async () => {});
	mockClient({
		list: vi.fn(async () => [row]),
		get: vi.fn(async () => ({
			id: 'ind-1',
			title: 'Doomed Indicator',
			slug: 'my-sma',
			source: '// x\n',
			compiled: true,
			compile_error: null,
			api_version: '0.0.0',
			updated_at: 0
		})),
		remove
	});

	await renderDock({ paneCount: 1, children: emptyChildren });
	await userEvent.click(page.getByText('Doomed Indicator'));
	await expect.poll(() => indicatorEditor.activeId).toBe('ind-1');

	await userEvent.click(page.getByRole('button', { name: 'Delete indicator' }));
	const confirmDialog = page.getByRole('alertdialog', { name: 'Delete indicator?' });
	await expect.element(confirmDialog).toBeVisible();
	await expect.element(confirmDialog.getByText('Doomed Indicator')).toBeInTheDocument();

	await userEvent.click(confirmDialog.getByRole('button', { name: 'Delete', exact: true }));
	await expect.poll(() => remove).toHaveBeenCalledWith('ind-1');
});

test('Cmd+S saves', async () => {
	const row = summary();
	const update = vi.fn(async (): Promise<SaveUserIndicatorResponse> => ({ id: 'ind-1', compiled: true, diagnostics: null }));
	mockClient({
		list: vi.fn(async () => [row]),
		get: vi.fn(async () => ({
			id: 'ind-1',
			title: 'My SMA',
			slug: 'my-sma',
			source: '// x\n',
			compiled: true,
			compile_error: null,
			api_version: '0.0.0',
			updated_at: 0
		})),
		update
	});

	await renderDock({ paneCount: 1, children: emptyChildren });
	await userEvent.click(page.getByText('My SMA'));
	await expect.poll(() => indicatorEditor.activeId).toBe('ind-1');

	const textarea = page.getByTestId('indicator-source-input');
	await userEvent.fill(textarea, '// x\n// edited\n');
	await textarea.click();
	await userEvent.keyboard('{Meta>}s{/Meta}');

	await expect.poll(() => update).toHaveBeenCalledTimes(1);
});

test('when the toolchain is unavailable, Save is disabled and the reason is visible', async () => {
	mockClient({}, { available: false, reason: 'no Rust toolchain on this machine' });

	await renderDock({ paneCount: 1, children: emptyChildren });
	await expect.element(page.getByText(/no Rust toolchain on this machine/)).toBeInTheDocument();

	const saveButton = page.getByRole('button', { name: 'SAVE' });
	await expect.element(saveButton).toHaveAttribute('aria-disabled', 'true');
});
