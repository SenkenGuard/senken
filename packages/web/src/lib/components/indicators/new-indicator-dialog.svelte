<script lang="ts">
	// "New indicator" — the one place a name and a starting template are
	// typed, per 034's "semua masukan lewat dialog" rule (no inline toolbar
	// input). `onOpenAutoFocus` puts the caret in the name field the moment
	// this opens, the same pattern `command-palette.svelte` uses for its own
	// search input.
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { cn } from '$lib/utils.js';
	import { INDICATOR_TEMPLATES, type IndicatorTemplateId } from './templates';

	let {
		open,
		creating,
		onCreate,
		onOpenChange
	}: {
		open: boolean;
		creating: boolean;
		onCreate: (title: string, template: IndicatorTemplateId) => void;
		onOpenChange: (open: boolean) => void;
	} = $props();

	let name = $state('');
	let template = $state<IndicatorTemplateId>('sma');
	let nameInput = $state<HTMLInputElement | null>(null);

	$effect(() => {
		if (open) {
			name = '';
			template = 'sma';
		}
	});

	function submit(e: SubmitEvent) {
		e.preventDefault();
		const title = name.trim();
		if (!title || creating) return;
		onCreate(title, template);
	}
</script>

<Dialog.Root {open} {onOpenChange}>
	<Dialog.Content
		onOpenAutoFocus={(e) => {
			e.preventDefault();
			nameInput?.focus();
		}}
		class="w-[380px] max-w-[92%] gap-4 border-ink/18 bg-card2"
	>
		<Dialog.Header>
			<Dialog.Title class="font-mono text-[13px] tracking-[0.03em]">New indicator</Dialog.Title>
			<Dialog.Description class="text-[11px] text-dim2">
				Name it, pick a starting point, and write the rest in Rust.
			</Dialog.Description>
		</Dialog.Header>

		<form onsubmit={submit} class="flex flex-col gap-3.5">
			<div class="flex flex-col gap-1.5">
				<Label for="new-indicator-name" class="font-mono text-[9px] tracking-[0.16em] text-dim2">NAME</Label>
				<Input
					id="new-indicator-name"
					bind:ref={nameInput}
					bind:value={name}
					placeholder="My SMA"
					autocomplete="off"
					class="h-8 text-[12px]"
				/>
			</div>

			<div class="flex flex-col gap-1.5">
				<span class="font-mono text-[9px] tracking-[0.16em] text-dim2">TEMPLATE</span>
				<div class="flex border border-ink/14">
					{#each INDICATOR_TEMPLATES as t (t.id)}
						<button
							type="button"
							class={cn(
								'flex-1 cursor-pointer py-1.5 font-mono text-[10px] tracking-[0.08em]',
								template === t.id ? 'bg-foreground text-inv' : 'text-dim2 hover:text-foreground'
							)}
							onclick={() => (template = t.id)}
						>
							{t.label.toUpperCase()}
						</button>
					{/each}
				</div>
			</div>

			<Dialog.Footer class="mt-1 gap-2">
				<Button type="button" variant="outline" onclick={() => onOpenChange(false)}>Cancel</Button>
				<Button type="submit" disabled={!name.trim() || creating}>
					{creating ? 'Creating…' : 'Create'}
				</Button>
			</Dialog.Footer>
		</form>
	</Dialog.Content>
</Dialog.Root>
