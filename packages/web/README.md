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
  routes (the command palette and AI panel's open/closed state, and
  `trade.svelte.ts`: the registered trade adapters, the accounts this user
  has attached, each one's resolved access and health, and the one shared
  copy of every account's portfolio — balances, positions, orders, fills —
  that the engine page and the charts page's order ticket both read and
  neither owns).
- `src/lib/trade/` — the trade engine's client half: `scaled.ts` (formatting
  and parsing the `{ scale, value }` pairs the API speaks, as strings —
  never through a `Number`), `form.ts` (an adapter's declared settings
  schema turned into form state and back), `view.ts` (the engine and
  dashboard pages' view models — including the currency-safe totals in
  `sumByCurrency`/`aggregateStats`/`dashboardEquity`, since there is no FX
  rate anywhere in this system to blend one currency into another),
  `poller.ts` (the auto-refresh timer's interval/visibility/no-overlap
  rules, tested against fake timers), `watch-scope.ts` (what the current
  set of mounted trade screens need refreshed, so the timer's own tick can
  ask for only that), and `portfolio-refresh.ts` (refreshing every
  currently-attached account's portfolio, always re-reading the account
  list fresh rather than one captured when a screen mounted).
- `src/lib/charts/trade-lines.ts` — turns an account's open positions and
  working orders into the entry/stop/target lines `chart-pane.svelte` draws
  on the price axis; lives beside the chart's other pure view-model modules
  rather than under `src/lib/trade/` because nothing else there touches
  chart layout.
- `src/lib/components/trade/` — the dynamic form builder a plugin's declared
  settings and actions are rendered through. A plugin ships data, never
  markup.
- `src/lib/mock/` — what is left of the local fixtures. Most pages read the
  real API now, including the home page's equity/positions/risk widgets,
  which read the same `tradeStore` portfolios the engine page does through
  `lib/trade/view.ts`'s `dashboardEquity`/`dashboardPositions`/`dashboardRisk`
  — the dashboard has no fabricated equity curve or fabricated positions
  left in it. Each remaining fixture documents at its own definition why it
  has no server field yet.
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
