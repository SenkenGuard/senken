// Global AI panel overlay state.
//
// reference: `state.aiOpen` / `state.aiMode` (lines 1504-1505) plus the
// derived `aiFloatOpen` / `aiSideOpen` / `aiClosed` flags (lines 3184-3212).
// Like `command-palette.svelte.ts`, this is a module-level rune store rather
// than context — see that file's header comment for the reasoning, which
// applies identically here: `layout/ai-panel.svelte` is mounted once in
// `AppShell`, so every route already sits under it, and nothing outside
// `AppShell` itself needs to open the panel (there is no per-route trigger
// the way the command palette has one on every page — the panel's own
// closed-state launcher chip is its trigger).
export type AiMode = 'float' | 'sidebar';

class AiPanelStore {
	open = $state(false);
	mode = $state<AiMode>('float');
}

export const aiPanel = new AiPanelStore();
