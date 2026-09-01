<script lang="ts">
	// "Go to date" — the reader's own control for `historyWindowAround`
	// (`$lib/charts/chart-window.ts`). Reached from the chart's own
	// right-click menu ("GO TO DATE…", `routes/charts/+page.svelte`), the
	// same place "RESET CHART VIEW" and "REFRESH CHART DATA" already live —
	// this is another way of changing what a pane shows, not a new class of
	// control.
	//
	// A native `<input type="date">` rather than a custom calendar: the
	// dialog only needs one value, out of a range wide enough that a custom
	// picker would need its own decade/month navigation to be usable at all,
	// and the browser's own control already does that.
	import * as Dialog from '$lib/components/ui/dialog/index.js';

	let {
		open,
		onClose,
		onJump
	}: {
		open: boolean;
		onClose: () => void;
		/** Fires with the picked date at UTC midnight, in Unix nanoseconds —
		 * `resolveJumpWindow`'s own unit. */
		onJump: (targetNanos: number) => void;
	} = $props();

	let dateText = $state('');

	function submit() {
		if (!dateText) return;
		const parsed = Date.parse(`${dateText}T00:00:00Z`);
		if (Number.isNaN(parsed)) return;
		onJump(parsed * 1_000_000);
		onClose();
	}
</script>

<Dialog.Root
	{open}
	onOpenChange={(v) => {
		if (!v) onClose();
	}}
>
	<Dialog.Content
		showCloseButton={false}
		class="top-[40%] w-[320px] max-w-[92%] gap-0 border-ink/18 bg-card2 p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		<Dialog.Header class="flex-row items-center justify-between gap-3 space-y-0 px-4 py-[13px]">
			<Dialog.Title class="text-[13px] font-medium tracking-[0.03em] text-foreground">Go to date</Dialog.Title>
		</Dialog.Header>
		<Dialog.Description class="sr-only">Jump this pane to bars around a chosen date</Dialog.Description>

		<div class="flex flex-col gap-3 px-4 pt-[14px] pb-4">
			<input
				type="date"
				class="h-[30px] border border-ink/14 bg-transparent px-2.5 font-mono text-[11px] text-foreground outline-none focus:border-foreground"
				bind:value={dateText}
				onkeydown={(e) => e.key === 'Enter' && submit()}
			/>
		</div>

		<div class="flex flex-none items-center justify-end gap-[7px] border-t border-ink/10 px-4 py-[11px]">
			<button type="button" class="cursor-pointer border border-ink/14 px-4 py-[7px]" onclick={onClose}>
				<span class="font-mono text-[9px] tracking-[0.18em] text-dim2">CANCEL</span>
			</button>
			<button
				type="button"
				disabled={!dateText}
				class="cursor-pointer bg-foreground px-[18px] py-[7px] disabled:cursor-not-allowed disabled:opacity-40"
				onclick={submit}
			>
				<span class="font-mono text-[9px] font-semibold tracking-[0.18em] text-inv">GO</span>
			</button>
		</div>
	</Dialog.Content>
</Dialog.Root>
