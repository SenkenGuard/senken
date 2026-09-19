<script lang="ts">
	// The dock's source-editing surface: a plain textarea with a line-number
	// gutter, purely presentational (`source`/`diagnostics` are props, never
	// fetched here) so it can be exercised with `svelte/server` the same way
	// `history-edge-status.svelte`/`widget-frame.svelte` are — real rendered
	// markup, not the logic that is supposed to produce it. Carried over from
	// the old authoring modal's `indicator-panel-editor.svelte` (same gutter/
	// textarea pairing, proven correct by its own tests), generalised from a
	// single `error` to a list of `diagnostics` — `rustc` can report more
	// than one error-level diagnostic per failed build, and every failing
	// line needs to be marked, not just the first.
	//
	// The gutter and the textarea share one font and line-height and neither
	// scrolls independently (the wrapping panel does), which is what keeps a
	// given source line's number lined up with its text without any
	// scroll-sync JavaScript — `rows={lines.length}` grows the textarea to
	// exactly the gutter's own row count.
	//
	// `Tab` inserts two spaces instead of moving focus out of the textarea
	// (a plain textarea's native behaviour) — the one piece of editor
	// convenience this component adds beyond a plain textarea with a gutter.
	// `Cmd/Ctrl+S` is handled by the caller (`indicator-dock.svelte`), not
	// here: this component owns no network call and no save action.
	import type { UserIndicatorDiagnosticDto } from '$lib/api/types';

	let {
		source,
		diagnostics = [],
		onSourceChange,
		onSave
	}: {
		source: string;
		diagnostics?: UserIndicatorDiagnosticDto[];
		onSourceChange: (value: string) => void;
		/** `Cmd/Ctrl+S` inside the editor — `stopPropagation`'d here so the
		 * same keypress is not also handled by `+page.svelte`'s global
		 * keydown handler. */
		onSave: () => void;
	} = $props();

	const lines = $derived(source.split('\n'));
	const errorLines = $derived(new Set(diagnostics.map((d) => d.line).filter((l): l is number => l !== null && l !== undefined)));
	const hasError = $derived(diagnostics.length > 0);

	function handleKeydown(e: KeyboardEvent) {
		if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 's') {
			e.preventDefault();
			e.stopPropagation();
			onSave();
			return;
		}
		if (e.key === 'Tab') {
			e.preventDefault();
			const el = e.currentTarget as HTMLTextAreaElement;
			const start = el.selectionStart;
			const end = el.selectionEnd;
			const next = source.slice(0, start) + '  ' + source.slice(end);
			onSourceChange(next);
			// The value write above is async (Svelte re-renders on the next
			// tick), so the caret has to be restored after it, not here.
			queueMicrotask(() => {
				el.selectionStart = el.selectionEnd = start + 2;
			});
		}
	}
</script>

<div data-indicator-source-editor class="flex min-h-0 flex-1 flex-col gap-0">
	<div class="flex min-h-0 flex-1 overflow-auto border border-ink/14 bg-card2">
		<div
			data-indicator-editor-gutter
			class="flex-none select-none border-r border-ink/10 px-2 py-2 text-right font-mono text-[11px] leading-[18px] text-dim2"
		>
			{#each lines as _line, i (i)}
				{@const lineNumber = i + 1}
				{@const isErrorLine = errorLines.has(lineNumber)}
				<div data-line-number data-line={lineNumber} data-line-error={isErrorLine} class={isErrorLine ? 'text-loss' : ''}>
					{lineNumber}
				</div>
			{/each}
		</div>
		<textarea
			data-indicator-editor-input
			data-testid="indicator-source-input"
			aria-invalid={hasError}
			aria-label="Indicator source"
			class="min-h-[140px] flex-1 resize-none bg-transparent px-2.5 py-2 font-mono text-[11px] leading-[18px] text-foreground outline-none focus-visible:ring-[3px] focus-visible:ring-ring/50"
			rows={Math.max(lines.length, 6)}
			spellcheck="false"
			autocomplete="off"
			autocapitalize="off"
			value={source}
			oninput={(e) => onSourceChange(e.currentTarget.value)}
			onkeydown={handleKeydown}
		></textarea>
	</div>
	<!-- Reserved even when there is nothing to show, at a fixed one-line
	     height, so the dock's own height never jumps the moment a
	     diagnostic appears or clears. -->
	<div data-diagnostics-strip class="flex min-h-[26px] flex-none flex-col justify-center gap-1 border-t border-ink/10 px-2.5 py-1">
		{#each diagnostics as d, i (i)}
			<div data-compile-error data-line={d.line ?? ''} data-column={d.column ?? ''} class="truncate font-mono text-[10.5px] text-loss">
				{#if d.line}Line {d.line}{#if d.column}, column {d.column}{/if}: {/if}{d.message}
			</div>
		{/each}
	</div>
</div>
