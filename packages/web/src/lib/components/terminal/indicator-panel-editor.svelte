<script lang="ts">
	// The indicator-authoring panel's editor surface: a plain textarea with a
	// line-number gutter, purely presentational (`source`/`error` are props,
	// never fetched here) so it can be exercised with `svelte/server` the
	// same way `history-edge-status.svelte`/`widget-frame.svelte` are — real
	// rendered markup, not the logic that is supposed to produce it.
	//
	// The gutter and the textarea share one font and line-height and neither
	// scrolls independently (the wrapping panel does), which is what keeps a
	// given source line's number lined up with its text without any
	// scroll-sync JavaScript — `rows={lines.length}` grows the textarea to
	// exactly the gutter's own row count.
	//
	// A compile error is shown two ways, both driven by the same `error`
	// prop: the offending line's own gutter cell gets `data-line-error`, and
	// a banner beneath restates the line, column and message in full — the
	// server sends `line`/`column`/`message` as they are (never collapsed
	// into a generic string), and both are shown here exactly as they came,
	// on purpose: the person reading this wrote the source, not the server.
	export interface CompileErrorInfo {
		line: number;
		column: number;
		message: string;
	}

	let {
		source,
		error = null,
		onSourceChange
	}: {
		source: string;
		error?: CompileErrorInfo | null;
		onSourceChange: (value: string) => void;
	} = $props();

	const lines = $derived(source.split('\n'));
</script>

<div data-indicator-editor class="flex flex-col gap-2.5">
	<div class="flex overflow-hidden border border-ink/14 bg-card2">
		<div
			data-indicator-editor-gutter
			class="flex-none select-none border-r border-ink/10 px-2 py-2 text-right font-mono text-[11px] leading-[18px] text-dim2"
		>
			{#each lines as _line, i (i)}
				{@const lineNumber = i + 1}
				{@const hasError = error?.line === lineNumber}
				<div data-line-number data-line={lineNumber} data-line-error={hasError} class={hasError ? 'text-red-400' : ''}>
					{lineNumber}
				</div>
			{/each}
		</div>
		<textarea
			data-indicator-editor-input
			class="min-h-[140px] flex-1 resize-none bg-transparent px-2.5 py-2 font-mono text-[11px] leading-[18px] text-foreground outline-none"
			rows={Math.max(lines.length, 6)}
			spellcheck="false"
			autocomplete="off"
			autocapitalize="off"
			value={source}
			oninput={(e) => onSourceChange(e.currentTarget.value)}
		></textarea>
	</div>
	{#if error}
		<div
			data-compile-error
			data-line={error.line}
			data-column={error.column}
			class="border border-red-500/40 bg-red-500/10 px-2.5 py-2 font-mono text-[10.5px] text-red-300"
		>
			Line {error.line}, column {error.column}: {error.message}
		</div>
	{/if}
</div>
