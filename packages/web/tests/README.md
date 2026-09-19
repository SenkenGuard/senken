# Web tests — three kinds, and when to reach for which

- **`*.test.ts`, colocated in `src/`** — pure logic, run by `bun test` (no
  DOM). One function, one return value, checked once. Nothing here can
  mount a component, click, or wait for an effect to settle.
- **`*.browser-test.ts`, colocated in `src/`** — component/reactive tests,
  run by `bun run test:browser` (Vitest Browser Mode, real Chromium via
  Playwright). Use these for a click, a keypress, a dialog's open/close
  cycle, or an effect that must be watched across several ticks (an
  infinite-fetch-loop bug looks identical to a correct effect if called
  only once). Named `*.browser-test.ts`, not `*.browser.test.ts` — `bun
  test`'s own patterns (`*.test.ts`, `*_test.*`, `*.spec.*`) would
  otherwise pick these up and fail against a server-render preload with no
  real DOM.
- **`tests/e2e/*.spec.ts`** — end-to-end, run by `bun run test:e2e`
  (Playwright against a real `senken serve` on a temporary data directory —
  never a mock, never the dev proxy). Use these for anything that needs a
  real login, a real account, or a real HTTP round trip the app itself
  makes. `bun test`'s root is scoped to `src` (see the root `test` script)
  specifically so these files are never picked up by it either.

## Running them

```bash
bun run --filter web test           # bun test src — pure logic
bun run --filter web test:browser   # Vitest Browser Mode
bun run --filter web test:e2e       # Playwright — run `cargo build --bin senken` first
```

## The probe rule

Every browser-mode and e2e test session checks
`document.visibilityState === 'visible'` and that one
`requestAnimationFrame` actually fires. A pane that never paints leaves
`requestAnimationFrame` dead, which leaves bits-ui's own unmount gate dead,
which reads as "the dialog is stuck open forever" — a harness artifact, not
an app bug, and one that has already fooled three QA sessions. Component
lifecycle observations (a dialog closing, focus landing somewhere) are only
meaningful once this probe has passed. It never runs inside `bun test`,
which has no document to probe at all.
