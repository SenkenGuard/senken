// Interactive proof for the picker's Enter/click behaviour. Nothing among
// the 64 existing `bun test` files can see this at all: it needs a real
// keyboard event dispatched at a real focused input inside a real browser,
// which is exactly what `svelte/server`'s one-shot render cannot do.
//
// A note on what this file actually found, since it differs from what it
// set out to prove: manual testing on 2026-09-03 reported that a real
// Enter key, inside the full charts page, left the palette open with
// `onPick` never called. Mounting only `CommandPalette` — one row, several
// rows, with and without `kindTabs` — and driving it with
// `userEvent.keyboard('{Enter}')` does not reproduce that: `onPick` fires
// exactly once and the dialog leaves the DOM, every time, across repeated
// runs. A test should not be bent to match an expected result, so this
// stays a passing test of the component in isolation, and the divergence
// from the manual finding is reported as-is rather than forced red.
import { test, expect, vi, afterEach } from 'vitest';
import { page, userEvent } from 'vitest/browser';
import { render } from 'vitest-browser-svelte';
import CommandPalette from './command-palette.svelte';
import { openCommand, closeCommand, commandPalette } from '$lib/state/command-palette.svelte';
import SigmaIcon from '@lucide/svelte/icons/sigma';

afterEach(() => {
	// Belt and braces: a failed assertion above must not leave the palette
	// open for the next test in this file.
	commandPalette.open = false;
});

/** One row, shaped exactly like the charts page's own "INDICATORS & LAYERS"
 * palette builds for its catalogue entries (`routes/charts/+page.svelte`'s
 * `openLayerPicker`, `catalogRows`) — where picking a row both runs its own
 * effect and calls `closeCommand()` itself; the palette component never
 * closes on a caller's behalf. */
function openWithOneRow(onPick: () => void): void {
	openCommand({
		mode: 'layer',
		placeholder: 'Search instruments or indicators…',
		footer: 'ADDING TO PANE 1',
		rows: () => [
			{
				icon: SigmaIcon,
				title: 'SMA',
				sub: 'SMA {period}',
				meta: 'OVERLAY',
				metaTone: 'dim',
				onPick: () => {
					onPick();
					closeCommand();
				}
			}
		]
	});
}

test('pressing Enter applies the highlighted row and closes the palette', async () => {
	const onPick = vi.fn();
	await render(CommandPalette);
	openWithOneRow(onPick);

	await userEvent.keyboard('SMA');
	const option = page.getByRole('option');
	await expect.element(option).toHaveAttribute('aria-selected', 'true');

	await userEvent.keyboard('{Enter}');

	// See this file's header: this is green in isolation, which disagrees
	// with the manual finding reported there. Reported, not hidden.
	expect(onPick).toHaveBeenCalledTimes(1);
	await expect.element(page.getByRole('dialog'), { timeout: 1000 }).not.toBeInTheDocument();
});

test('clicking the highlighted row applies it and closes the palette', async () => {
	const onPick = vi.fn();
	await render(CommandPalette);
	openWithOneRow(onPick);

	await userEvent.keyboard('SMA');
	const option = page.getByRole('option');
	await expect.element(option).toHaveAttribute('aria-selected', 'true');

	await userEvent.click(option);

	// The control: this path already works today (verified live: synthetic
	// and real clicks both succeed), so this test stays green regardless of
	// the keyboard-Enter fix above — it exists to prove the harness itself,
	// not just the bug.
	expect(onPick).toHaveBeenCalledTimes(1);
	await expect.element(page.getByRole('dialog')).not.toBeInTheDocument();
});

test('two rows with the same title and sub still resolve independently, by id', async () => {
	// bits-ui requires `Command.Item.value` to be unique; the fallback
	// (`title + ' ' + sub`) collides for two rows like these — a built-in
	// indicator and a same-named user-authored one will look exactly like
	// this once dynamic indicators exist. `id` (a real, caller-supplied
	// identity — the indicator's own catalogue name, in practice) is what
	// keeps them distinguishable.
	const onPickFirst = vi.fn();
	const onPickSecond = vi.fn();
	await render(CommandPalette);
	openCommand({
		mode: 'layer',
		placeholder: 'Search instruments or indicators…',
		footer: 'ADDING TO PANE 1',
		rows: () => [
			{
				id: 'first',
				icon: SigmaIcon,
				title: 'SMA',
				sub: 'SMA {period}',
				meta: 'OVERLAY',
				metaTone: 'dim' as const,
				onPick: () => {
					onPickFirst();
					closeCommand();
				}
			},
			{
				id: 'second',
				icon: SigmaIcon,
				title: 'SMA',
				sub: 'SMA {period}',
				meta: 'OVERLAY',
				metaTone: 'dim' as const,
				onPick: () => {
					onPickSecond();
					closeCommand();
				}
			}
		]
	});

	const options = page.getByRole('option');
	await expect.element(options.first()).toBeInTheDocument();
	await userEvent.keyboard('{ArrowDown}');
	await expect.element(options.nth(1)).toHaveAttribute('aria-selected', 'true');

	await userEvent.keyboard('{Enter}');

	expect(onPickSecond).toHaveBeenCalledTimes(1);
	expect(onPickFirst).not.toHaveBeenCalled();
});

// The search input's `focus-visible:outline-*` classes are not checked here
// with `getComputedStyle`: this file mounts `CommandPalette` on its own, and
// nothing in that isolated mount ever imports the app's real stylesheet
// (`routes/+layout.svelte`'s own `layout.css`, which is where Tailwind's
// generated utilities actually live) — a computed-style assertion here would
// pass on the browser's own default focus ring whether or not the class
// existed at all, exactly the "Tailwind reports nothing for a class that
// does not exist" trap `AGENTS.md` warns about. `tests/e2e/charts.spec.ts`'s
// "the indicator picker's search input shows a focus-visible outline" checks
// it against the real, fully-styled page instead.
