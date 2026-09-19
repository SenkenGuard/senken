// Interactive proof that the native `window.prompt` this bar used to shell
// out to for renaming is gone, and that deleting a workspace needs an
// actual click on the destructive button — not just Enter landing wherever
// focus happens to be. A destructive action needs an explicit confirmation,
// never a bare keypress.
import { test, expect, vi } from 'vitest';
import { page, userEvent } from 'vitest/browser';
import { render } from 'vitest-browser-svelte';
import DashboardWorkspaceBar from './dashboard-workspace-bar.svelte';
import type { DashboardWorkspaceDto } from './api';

function workspace(id: string, name: string): DashboardWorkspaceDto {
	return { id, owner_id: 'u1', name, columns: 6, revision: 1, created_at: 0, updated_at: 0 };
}

const TWO_WORKSPACES: DashboardWorkspaceDto[] = [workspace('w1', 'Default'), workspace('w2', 'Scalping')];

async function openMenu(): Promise<void> {
	await userEvent.click(page.getByRole('button', { name: 'Workspace menu' }));
	await expect.element(page.getByRole('menu')).toBeInTheDocument();
}

test('renaming through the dialog with Enter calls onRename, never window.prompt', async () => {
	const promptSpy = vi.spyOn(window, 'prompt');
	const onRename = vi.fn();

	await render(DashboardWorkspaceBar, {
		workspaces: TWO_WORKSPACES,
		activeId: 'w1',
		activeWidgetCount: 0,
		onSelect: () => {},
		onAdd: () => {},
		onOpenAddWidget: () => {},
		onRename,
		onDelete: () => {}
	});

	await openMenu();
	await userEvent.click(page.getByText('RENAME WORKSPACE…'));

	const input = page.getByLabelText('Workspace name');
	await expect.element(input).toBeInTheDocument();
	await userEvent.clear(input);
	await userEvent.type(input, 'Baru');
	await userEvent.keyboard('{Enter}');

	expect(onRename).toHaveBeenCalledTimes(1);
	expect(onRename).toHaveBeenCalledWith('w1', 'Baru');
	expect(promptSpy).not.toHaveBeenCalled();
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
});

test('Escape cancels the rename dialog without calling onRename', async () => {
	const onRename = vi.fn();

	await render(DashboardWorkspaceBar, {
		workspaces: TWO_WORKSPACES,
		activeId: 'w1',
		activeWidgetCount: 0,
		onSelect: () => {},
		onAdd: () => {},
		onOpenAddWidget: () => {},
		onRename,
		onDelete: () => {}
	});

	await openMenu();
	await userEvent.click(page.getByText('RENAME WORKSPACE…'));
	const input = page.getByLabelText('Workspace name');
	await expect.element(input).toBeInTheDocument();

	await userEvent.keyboard('{Escape}');

	expect(onRename).not.toHaveBeenCalled();
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
});

test('deleting a workspace needs a real click on the destructive button, not just Enter', async () => {
	const onDelete = vi.fn();

	await render(DashboardWorkspaceBar, {
		workspaces: TWO_WORKSPACES,
		activeId: 'w2',
		activeWidgetCount: 4,
		onSelect: () => {},
		onAdd: () => {},
		onOpenAddWidget: () => {},
		onRename: () => {},
		onDelete
	});

	await openMenu();
	await userEvent.click(page.getByText('DELETE WORKSPACE'));

	const dialog = page.getByRole('alertdialog');
	await expect.element(dialog).toBeInTheDocument();
	await expect.element(page.getByText('Delete workspace Scalping? Its 4 widgets will be removed.')).toBeInTheDocument();

	// Enter's default target inside an alert dialog is the safe Cancel
	// button, not the destructive one — pressing it must not delete
	// anything.
	await userEvent.keyboard('{Enter}');
	expect(onDelete).not.toHaveBeenCalled();

	await userEvent.click(page.getByRole('button', { name: 'Delete' }));
	expect(onDelete).toHaveBeenCalledTimes(1);
	expect(onDelete).toHaveBeenCalledWith('w2');
	await expect.element(page.getByRole('alertdialog')).not.toBeInTheDocument();
});

test('the last remaining workspace cannot be deleted, and the reason is on the button', async () => {
	const onDelete = vi.fn();

	await render(DashboardWorkspaceBar, {
		workspaces: [workspace('w1', 'Default')],
		activeId: 'w1',
		activeWidgetCount: 0,
		onSelect: () => {},
		onAdd: () => {},
		onOpenAddWidget: () => {},
		onRename: () => {},
		onDelete
	});

	await openMenu();
	await userEvent.click(page.getByText('DELETE WORKSPACE'));

	const deleteButton = page.getByRole('button', { name: 'Delete' });
	await expect.element(deleteButton).toHaveAttribute('aria-disabled', 'true');
	await expect.element(deleteButton).toHaveAttribute('title', 'You need at least one workspace');

	// `aria-disabled` (not a real `disabled` attribute — see
	// `confirm-dialog.svelte`'s own note on why) already makes Playwright's
	// own actionability check refuse a normal `userEvent.click` here, which
	// is the point: a real user's mouse cannot reach this button either.
	// This asserts the click *handler's* own guard holds too, for whatever
	// still manages to fire a `click` event on it (a screen reader's
	// synthesized activation, for one).
	deleteButton.element().dispatchEvent(new MouseEvent('click', { bubbles: true }));
	expect(onDelete).not.toHaveBeenCalled();
});
