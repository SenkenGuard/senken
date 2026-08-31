<script lang="ts">
	// The shared chrome around every route: TopBar above, NavRail beside the routed page,
	// FooterBar below, plus the two global overlays that reach
	// every route: the AI panel (float/launcher, lines 882-919, plus the
	// sidebar variant, lines 1234-1263) and the command palette
	// (lines 1141-1177) — both mounted once, here, per Part C ("global
	// chrome... in layout/").
	//
	// also the auth gate. `/login` is the one route with no
	// chrome of its own; every other route requires a stored credential for
	// the active server. The `{#if}` below is the whole mechanism — it
	// renders *either* the login page's children *or* the terminal shell,
	// never both, and never the shell before `authorized` is true. That is
	// what prevents the flash-of-terminal-UI bug this pattern is known for:
	// there is no third branch that mounts the shell "temporarily" while a
	// check resolves, the way an `{#await}` or a post-mount `$effect` guard
	// would. `sessionStore.hasCredential` (`$lib/api/session.svelte.ts`) is a
	// synchronous read of already-loaded state (mirrored from
	// `localStorage`), not an awaited request, so the very first render
	// already reflects it — nothing here is "correct after a tick."
	//
	// A stored credential is only ever a *plausible* session, not a proven
	// one (proving it needs a round trip). `apiClient.startHeartbeat`'s poll
	// of `GET /api/me` (see its doc) is what actually validates it; a 401
	// there drives `sessionStore.hasCredential` back to `false` via
	// `handleSessionExpired`, and the `$effect` below reacts to that just
	// like it reacts to the first render. So a stale/forged token can cause
	// a brief flash of the *real* shell before the next heartbeat tick
	// corrects it, but never the reverse (there is no path that shows the
	// shell to someone this component has already determined has no
	// credential at all) — the case B4/B8 actually cares about ("do not
	// assume access before checking") is closed; the residual window is a
	// consequence of authentication being fundamentally asynchronous, not a
	// gap in this gate.
	import type { Snippet } from 'svelte';
	import { goto } from '$app/navigation';
	import { page } from '$app/state';
	import { QueryClientProvider } from '@tanstack/svelte-query';
	import TitleBar from './title-bar.svelte';
	import TopBar from './top-bar.svelte';
	import NavRail from './nav-rail.svelte';
	import FooterBar from './footer-bar.svelte';
	import AiPanel from './ai-panel.svelte';
	import CommandPalette from './command-palette.svelte';
	import { Toaster } from '$lib/components/ui/sonner/index.js';
	import { queryClient } from '$lib/api/query';
	import { apiClient, setSessionExpiredHandler } from '$lib/api/client';
	import { primeSessionPresence, sessionStore } from '$lib/api/session.svelte';
	import { wsClient } from '$lib/api/websocket';

	let { children }: { children: Snippet } = $props();

	// Before the gate below reads it, and after every module in the
	// `servers`/`session` import cycle has finished initialising.
	primeSessionPresence();

	const isLoginRoute = $derived(page.url.pathname === '/login');
	const authorized = $derived(sessionStore.hasCredential);

	// B16 point 2: "route to login exactly once" — wired here, once, so a
	// session that dies while the user is on *any* route (not just ones that
	// happen to import the login page) still gets routed there. `AppShell`
	// mounts exactly once for the whole app (it is the root layout's only
	// child), so this assignment itself runs once, not per-route.
	setSessionExpiredHandler(() => {
		void goto('/login', { replaceState: true });
	});

	// the connection layer starts watching as soon as the shell
	// mounts (see `apiClient.startHeartbeat`'s doc for what it polls and
	// why) — regardless of route or auth state, so the login page's
	// reachability indicator and insecure-connection warning have live data
	// too, not just the authenticated shell's `connection-status.svelte`.
	$effect(() => {
		apiClient.startHeartbeat();
		return () => apiClient.stopHeartbeat();
	});

	// the live-price socket only means anything once a session
	// exists (`POST /api/ws/ticket` requires one) — gated on `authorized`,
	// unlike the heartbeat above, so a logged-out visitor on `/login` never
	// dials it. This effect's only reactive read is `authorized`, so it
	// re-runs (tearing the old connection down via its cleanup, then
	// dialling a fresh one) exactly on a login/logout transition, not on
	// every render.
	$effect(() => {
		if (authorized) {
			wsClient.connect();
			return () => wsClient.disconnect();
		}
	});

	// The redirect half of the gate — a side effect, deliberately separate
	// from the render below. The render already shows the right thing for
	// both "matched" states (unauthenticated on `/login`; authenticated
	// elsewhere) synchronously, so there is never a painted frame this
	// effect needs to replace. It only fires for the two *mismatched*
	// states — an authenticated visitor sitting on `/login`, or an
	// unauthenticated one anywhere else — neither of which the render below
	// shows anything for at all (see the trailing `{:else}`).
	$effect(() => {
		if (isLoginRoute && authorized) {
			void goto('/', { replaceState: true });
		} else if (!isLoginRoute && !authorized) {
			void goto('/login', { replaceState: true });
		}
	});
</script>

{#if isLoginRoute && !authorized}
	<!-- `TitleBar` still renders here (it is a no-op `display: none` outside
	     the Tauri shell — see its own module doc) because it is also the
	     desktop window's drag region: without it, the `gui` build would have
	     no way to move or double-click-maximise the window while sitting on
	     `/login`, since nothing else here carries `data-tauri-drag-region`. -->
	<TitleBar />
	{@render children()}
{:else if !isLoginRoute && authorized}
	<QueryClientProvider client={queryClient}>
		<!--
			`TitleBar` is a full-width row of its own, above everything else,
			because the desktop window's chrome spans the whole window — and
			because `AiPanel`, docked beside the app in sidebar mode, must
			begin *below* it rather than beside it. Nesting the title bar
			inside the left column (as this once did) made the AI sidebar run
			the full height of the window and sit level with the traffic
			lights instead of with `TopBar`.
		-->
		<div
			class="flex h-screen w-full flex-col overflow-hidden bg-background text-secondary-foreground"
		>
			<TitleBar />
			<div class="flex min-h-0 min-w-0 flex-1">
				<div class="flex min-h-0 min-w-0 flex-1 flex-col">
					<TopBar />
					<div class="flex min-h-0 flex-1">
						<NavRail />
						<div class="flex min-h-0 min-w-0 flex-1 flex-col">
							{@render children()}
						</div>
					</div>
					<FooterBar />
				</div>
				<AiPanel />
			</div>
		</div>

		<CommandPalette />
	</QueryClientProvider>
{:else}
	<!-- The two mismatched states above, between the initial render and the
	     `$effect`'s `goto` landing. Never the shell, never the login form —
	     see the module doc for why that is the property that matters here. -->
	<div class="flex h-screen w-full items-center justify-center bg-background">
		<span class="font-mono text-[10px] tracking-[0.2em] text-dim">SENKEN</span>
	</div>
{/if}

<!-- One toaster for the whole app, outside every branch above so it survives
     the login/authenticated switch and a route change — the single place a
     transient failure (a rejected layout save, and anything else that
     reaches for `toast` in future) surfaces, rather than each route growing
     its own inline banner. -->
<Toaster />
