<script lang="ts">
	// A generic confirmation dialog for a destructive action — replaces
	// `window.confirm` wherever this app asks "are you sure?" before deleting
	// something. Built on `AlertDialog` (the same primitive
	// `settings/sections/storage-section.svelte` already uses for deleting
	// stored data), not the plain `Dialog` the rest of this app's non-
	// destructive dialogs use: `AlertDialog` is the one bits-ui traps focus
	// and Escape for specifically around a decision the user cannot dismiss
	// by clicking outside it without choosing.
	//
	// The confirm button is deliberately not `AlertDialog.Action` (which
	// auto-closes on click): `onConfirm` is often asynchronous (a network
	// delete), and this dialog does not know whether its caller wants it to
	// stay open on failure — closing is left entirely to the caller changing
	// `open` back to `false`, the same division `storage-section.svelte`
	// already uses its own plain `Button` for.
	//
	// `confirmDisabled` never sets the button's real `disabled` attribute:
	// a genuinely disabled button stops receiving pointer events in most
	// browsers, which would make `confirmDisabledReason` silently
	// undiscoverable on hover — exactly the "disabled with no visible
	// reason" this app's own maturity bar rules out. `aria-disabled` plus a
	// guarded click handler keeps the button hoverable while still
	// preventing the action.
	import * as AlertDialog from './alert-dialog/index.js';
	import { Button } from './button/index.js';
	import { cn } from '$lib/utils.js';

	let {
		open,
		title,
		description,
		confirmLabel = 'Delete',
		cancelLabel = 'Cancel',
		confirmDisabled = false,
		confirmDisabledReason,
		onConfirm,
		onOpenChange
	}: {
		open: boolean;
		title: string;
		/** The full sentence shown under the title — callers name the
		 * object being affected here ("Delete workspace *Scalping*? Its 4
		 * widgets will be removed."), never a generic "are you sure?". */
		description: string;
		confirmLabel?: string;
		cancelLabel?: string;
		/** `true` while the action this dialog confirms is not currently
		 * allowed (e.g. deleting the last remaining workspace) — the button
		 * stays visible and hoverable so `confirmDisabledReason` is
		 * discoverable, but a click does nothing. */
		confirmDisabled?: boolean;
		/** Shown as the confirm button's `title` tooltip whenever
		 * `confirmDisabled` is `true`. Required together with
		 * `confirmDisabled` so a disabled destructive action never has no
		 * visible reason. */
		confirmDisabledReason?: string;
		onConfirm: () => void;
		onOpenChange: (open: boolean) => void;
	} = $props();
</script>

<AlertDialog.Root {open} {onOpenChange}>
	<AlertDialog.Content>
		<AlertDialog.Header>
			<AlertDialog.Title>{title}</AlertDialog.Title>
			<AlertDialog.Description>{description}</AlertDialog.Description>
		</AlertDialog.Header>
		<AlertDialog.Footer>
			<AlertDialog.Cancel>{cancelLabel}</AlertDialog.Cancel>
			<Button
				variant="destructive"
				aria-disabled={confirmDisabled}
				title={confirmDisabled ? confirmDisabledReason : undefined}
				class={cn(confirmDisabled && 'cursor-not-allowed opacity-50')}
				onclick={() => {
					if (confirmDisabled) return;
					onConfirm();
				}}
			>
				{confirmLabel}
			</Button>
		</AlertDialog.Footer>
	</AlertDialog.Content>
</AlertDialog.Root>
