<script lang="ts">
	// A title bar of our own, drawn only inside the desktop shell.
	//
	// The reference has no such strip — a browser has no
	// window controls to accommodate. Earlier attempts folded the macOS traffic
	// lights into the 46px header instead, which never sat right: they landed
	// inside the brand cell and read as a hole punched into the chrome.
	//
	// A dedicated strip is what VS Code, Slack and Linear do, and it works
	// because the lights get a band that is theirs, level with nothing else.
	// The app's own chrome then starts below it, completely unmodified — which
	// is why the browser layout needs no compensation at all.
	//
	// Height and the traffic-light offset are kept in step with
	// `apps/senken/src/gui.rs`; see the comment there.
	//
	// `data-tauri-drag-region` only makes a window draggable
	// from the exact element the pointer hits — Tauri hit-tests the event
	// target, not its ancestors — so a press that lands on the "SENKEN"
	// glyphs themselves used to hit this `<span>`, which carried no such
	// attribute, and the drag silently never started. `pointer-events-none`
	// makes the span transparent to hit-testing so the press always resolves
	// to the parent div underneath, which does carry the attribute — simpler
	// than stamping the attribute on every text node, and correct here
	// specifically because this label is purely decorative (nothing under it
	// is ever meant to be clickable).
</script>

<div
	data-tauri-drag-region
	class="shell-title-bar flex-none items-center justify-center border-b border-ink/9 bg-chrome select-none"
>
	<span class="pointer-events-none font-mono text-[8px] tracking-[0.28em] text-dim">SENKEN</span>
</div>
