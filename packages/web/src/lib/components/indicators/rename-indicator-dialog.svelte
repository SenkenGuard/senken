<script lang="ts">
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';

	let {
		open,
		currentTitle,
		saving,
		onRename,
		onOpenChange
	}: {
		open: boolean;
		currentTitle: string;
		saving: boolean;
		onRename: (title: string) => void;
		onOpenChange: (open: boolean) => void;
	} = $props();

	let name = $state('');
	let nameInput = $state<HTMLInputElement | null>(null);

	$effect(() => {
		if (open) name = currentTitle;
	});

	function submit(e: SubmitEvent) {
		e.preventDefault();
		const title = name.trim();
		if (!title || saving) return;
		onRename(title);
	}
</script>

<Dialog.Root {open} {onOpenChange}>
	<Dialog.Content
		onOpenAutoFocus={(e) => {
			e.preventDefault();
			nameInput?.focus();
			nameInput?.select();
		}}
		class="w-[340px] max-w-[92%] gap-4 border-ink/18 bg-card2"
	>
		<Dialog.Header>
			<Dialog.Title class="font-mono text-[13px] tracking-[0.03em]">Rename indicator</Dialog.Title>
			<Dialog.Description class="sr-only">Give this indicator a new name</Dialog.Description>
		</Dialog.Header>

		<form onsubmit={submit} class="flex flex-col gap-3.5">
			<div class="flex flex-col gap-1.5">
				<Label for="rename-indicator-name" class="font-mono text-[9px] tracking-[0.16em] text-dim2">NAME</Label>
				<Input id="rename-indicator-name" bind:ref={nameInput} bind:value={name} autocomplete="off" class="h-8 text-[12px]" />
			</div>
			<Dialog.Footer class="mt-1 gap-2">
				<Button type="button" variant="outline" onclick={() => onOpenChange(false)}>Cancel</Button>
				<Button type="submit" disabled={!name.trim() || saving}>{saving ? 'Saving…' : 'Rename'}</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
