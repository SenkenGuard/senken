<script lang="ts">
	// Replaces `window.prompt('Rename workspace', …)` — a native prompt is
	// invisible to this app's own theme and cannot be driven or asserted on
	// in a browser test, which is why this app avoids native prompts
	// everywhere.
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';

	let {
		open,
		currentName,
		onRename,
		onOpenChange
	}: {
		open: boolean;
		currentName: string;
		onRename: (name: string) => void;
		onOpenChange: (open: boolean) => void;
	} = $props();

	let name = $state('');
	let inputEl = $state<HTMLInputElement | null>(null);

	// Seeds the input from whichever workspace is being renamed, once per
	// dialog open — not on every keystroke, since `name` becomes this
	// dialog's own working copy from then on. Read here, inside the effect,
	// rather than as `$state(currentName)`'s own initializer: that would
	// only ever capture the value `currentName` held the *first* time this
	// component happened to mount, not the workspace this open is actually
	// for.
	let seededFor = $state<string | null>(null);
	$effect(() => {
		if (open && seededFor !== currentName) {
			name = currentName;
			seededFor = currentName;
		}
		if (!open) seededFor = null;
	});

	const trimmed = $derived(name.trim());
	const canRename = $derived(trimmed.length > 0 && trimmed !== currentName);

	function submit(): void {
		if (!canRename) return;
		onRename(trimmed);
	}
</script>

<Dialog.Root {open} {onOpenChange}>
	<Dialog.Content
		class="w-[360px]"
		onOpenAutoFocus={(e) => {
			e.preventDefault();
			inputEl?.select();
		}}
	>
		<Dialog.Header>
			<Dialog.Title>Rename workspace</Dialog.Title>
		</Dialog.Header>
		<Input
			bind:ref={inputEl}
			bind:value={name}
			aria-label="Workspace name"
			onkeydown={(e: KeyboardEvent) => {
				if (e.key === 'Enter') {
					e.preventDefault();
					submit();
				}
			}}
		/>
		<Dialog.Footer>
			<Button variant="outline" onclick={() => onOpenChange(false)}>Cancel</Button>
			<Button disabled={!canRename} onclick={submit}>Rename</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>
