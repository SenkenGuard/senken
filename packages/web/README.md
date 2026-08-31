# web

Senken's terminal UI: a SvelteKit SPA (Svelte 5 runes, Tailwind 4, adapter-static)
built from the Claude Design reference.

- `src/routes/layout.css` — design tokens mapped onto
  shadcn's own variable names, plus `--radius: 0` and the Archivo/IBM Plex Mono
  font stack. Retheme through these tokens, not per-component overrides.
- `src/lib/components/ui/` — generic shadcn-svelte primitives (55) plus a
  handful of generic additions (`stat-cell`, `ticker-tape`, …). Nothing
  here may know what Senken is.
- `src/lib/components/layout/` — the app chrome: `AppShell`, `TopBar`,
  `NavRail`, `FooterBar`, and the two global overlays:
  `AiPanel` and `CommandPalette`, both driven by the module-level rune
  stores in `src/lib/state/` (see that folder and the plan's P5 report for
  why a store rather than Svelte context).
- `src/lib/components/terminal/` — domain widgets specific to one route,
  including the charts page's `chart-settings-dialog.svelte` and
  `layer-dialog.svelte` — reachable only from that route, so
  they keep their open/closed state locally rather than in `src/lib/state/`.
- `src/lib/state/` — module-level rune stores for state shared across
  routes (the command palette and AI panel's open/closed state, and the
  trade engine's reactive account list).
- `src/lib/mock/` — local fixtures for every page and overlay. This app
  makes no `fetch`/`/api` calls; every value on screen is a mock until a
  later plan wires up the runtime.
- Icons are **lucide** (`@lucide/svelte`) throughout, not the reference's
  Phosphor — shadcn is already configured for lucide (`components.json`) and
  it was already installed, so this avoids mixing two icon sets.

---

# sv

Everything you need to build a Svelte project, powered by [`sv`](https://github.com/sveltejs/cli).

## Creating a project

If you're seeing this, you've probably already done this step. Congrats!

```sh
# create a new project
npx sv create my-app
```

To recreate this project with the same configuration:

```sh
# recreate this project
bun x sv@0.17.0 create --template minimal --types ts --add tailwindcss="plugins:none" sveltekit-adapter="adapter:static" --no-download-check --install bun web
```

## Developing

Once you've created a project and installed dependencies with `npm install` (or `pnpm install` or `yarn`), start a development server:

```sh
npm run dev

# or start the server and open the app in a new browser tab
npm run dev -- --open
```

## Building

To create a production version of your app:

```sh
npm run build
```

You can preview the production build with `npm run preview`.

> To deploy your app, you may need to install an [adapter](https://svelte.dev/docs/kit/adapters) for your target environment.
