<script lang="ts">
	// The login page. Rendered with no chrome of its own — see
	// `app-shell.svelte`'s auth gate, which mounts this route's children
	// directly instead of inside TopBar/NavRail/FooterBar.
	//
	// Styled like the rest of the terminal (square — `--radius: 0` is global,
	// mono numerals, the same hairline-border/tracked-uppercase-label
	// vocabulary as `layout/top-bar.svelte` and `layout/footer-bar.svelte`),
	// not a generic centred card.
	import { apiClient } from '$lib/api/client';
	import { activeServer, isSecureConnection, resolveBaseUrl } from '$lib/api/servers.svelte';
	import { getErrorMessage } from '$lib/api/errors';
	import { MIN_PASSWORD_LENGTH } from '$lib/api/constants';
	import ServerPicker from './server-picker.svelte';
	import * as Tabs from '$lib/components/ui/tabs/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Label } from '$lib/components/ui/label/index.js';
	import { Spinner } from '$lib/components/ui/spinner/index.js';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';

	// Mirrors `senken_identity::store::DEFAULT_ADMIN_EMAIL`
	// (`crates/identity/src/store.rs`) — a fast-fail hint for the form,
	// never the source of truth; the server re-checks it independently.
	// Not exposed anywhere in `GET /api/openapi.json` (that document carries
	// request/response schemas, not seed data), so there is no generated
	// value to import instead. `MIN_PASSWORD_LENGTH`, the same kind of
	// constant, lives in `$lib/api/constants` — folded
	// there once this page and `settings-api.ts` turned out to have each
	// hand-copied their own copy of it independently.
	const DEFAULT_ADMIN_EMAIL = 'admin@mail.com';

	/**
	 * Which tab opens by default. the fresh-install experience is
	 * "show set a password, not log in".
	 *
	 * Q5 originally had no server-side way to ask "is this account fenced?"
	 * without submitting a real request — `crates/api/src/identity_handlers.rs`'s
	 * anonymous `set-password` path was the only anonymous read of fence
	 * state, and it requires a candidate password to even attempt, because a
	 * fenced and an already-set-up account must look identical to anyone who
	 * doesn't already hold a session (the same account-enumeration defence
	 * B15 already applies to `login`). Probing it with a throwaway password
	 * and inferring the answer from *which* of its two 400 messages comes
	 * back would have worked, but Q5 declined it as fragile — coupled to
	 * exact server wording that carries no compatibility guarantee — and
	 * used a client-side `localStorage` heuristic ("has this browser ever
	 * completed a login against this server?") instead, recommending
	 * exactly the endpoint below as the real fix.
	 *
	 * Q8 then added that endpoint: `GET /api/health`'s `needs_setup` field
	 * (`loadNeedsSetup`, below) reports the seeded default admin's real fence
	 * state directly, unauthenticated, without accepting or checking a
	 * password — no enumeration surface, because it always names the one
	 * fixed, already-public default admin account rather than a
	 * caller-supplied one. This page is wired to that field,
	 * replacing the heuristic: unlike `localStorage`, it is not wrong on a
	 * second browser or after clearing site data, because it asks the
	 * server instead of guessing from this browser's own history.
	 *
	 * Both tabs stay manually reachable regardless of what `needsSetup`
	 * says, and the fence itself is still enforced entirely server-side
	 * : picking the "wrong" tab does not grant anything, it just
	 * surfaces the server's own error message (`getErrorMessage`, below)
	 * instead of guessing right on the first try.
	 */
	let manualTab = $state<'login' | 'setup' | null>(null);
	/** `null` until the first `GET /api/health` for the active server
	 * resolves (or forever, if it never does — a briefly unreachable server
	 * is already surfaced elsewhere, by the connection indicator and
	 * heartbeat, so this falls back to `'login'` rather than adding a second,
	 * uncoordinated opinion about the same failure). */
	let needsSetup = $state<boolean | null>(null);
	let lastServerId = activeServer().id;

	/** Fetches `needsSetup` for `serverId` from the server itself
	 * See `tab`'s doc above for why this replaced a `localStorage`
	 * heuristic. `GET /api/health` needs no credential, so
	 * this can run before any session exists at all. */
	async function loadNeedsSetup(serverId: string): Promise<void> {
		try {
			const health = await apiClient.health();
			if (activeServer().id !== serverId) return; // superseded by a server switch
			needsSetup = health.needs_setup;
		} catch {
			// Leave `needsSetup` as `null` — see its own doc above.
		}
	}

	$effect(() => {
		const id = activeServer().id;
		if (id !== lastServerId) {
			lastServerId = id;
			manualTab = null;
			needsSetup = null;
		}
		void loadNeedsSetup(id);
	});
	const tab = $derived(manualTab ?? (needsSetup === true ? 'setup' : 'login'));

	// B15: "the client must warn when the chosen server is neither loopback
	// nor `https`... in the UI, not just a log line" — and per the brief,
	// prominently here, not only as `connection-status.svelte`'s small
	// top-bar badge (which this route doesn't even render — see
	// `app-shell.svelte`).
	const insecure = $derived(!isSecureConnection(activeServer()));

	let loginEmail = $state('');
	let loginPassword = $state('');
	let loginBusy = $state(false);
	let loginError = $state<string | null>(null);

	async function submitLogin(event: SubmitEvent) {
		event.preventDefault();
		loginBusy = true;
		loginError = null;
		try {
			await apiClient.login(loginEmail.trim(), loginPassword);
			// No explicit navigation from here: `app-shell.svelte`'s auth gate
			// reacts to the credential this just stored (`sessionStore`,
			// updated inside `apiClient.login`) and redirects away from
			// `/login` itself — see that component's module doc.
		} catch (error) {
			loginError = getErrorMessage(error, 'Could not log in — check your email and password.');
		} finally {
			loginBusy = false;
		}
	}

	let setupEmail = $state(DEFAULT_ADMIN_EMAIL);
	let setupPassword = $state('');
	let setupConfirm = $state('');
	let setupBusy = $state(false);
	let setupError = $state<string | null>(null);

	const setupMismatch = $derived(setupConfirm.length > 0 && setupPassword !== setupConfirm);
	const setupTooShort = $derived(setupPassword.length > 0 && setupPassword.length < MIN_PASSWORD_LENGTH);
	const setupCanSubmit = $derived(
		setupEmail.trim().length > 0 &&
			setupPassword.length >= MIN_PASSWORD_LENGTH &&
			setupPassword === setupConfirm &&
			!setupBusy
	);

	async function submitSetup(event: SubmitEvent) {
		event.preventDefault();
		if (!setupCanSubmit) return;
		setupError = null;
		setupBusy = true;
		try {
			const email = setupEmail.trim();
			// the only way a fenced account ever gets a password.
			// `setPasswordAnonymous` sends no `Authorization` header
			// regardless of whatever else this browser has stored
			// (`client.ts`'s `request` doc explains why that matters).
			await apiClient.setPasswordAnonymous(email, setupPassword);
			// The account is unfenced the instant the call above succeeds —
			// update the signal `tab` reads to match, so a re-render before
			// `app-shell.svelte`'s auth gate navigates away doesn't flip back
			// to this tab.
			needsSetup = false;
			// This second call is now a normal login, not a special case.
			await apiClient.login(email, setupPassword);
		} catch (error) {
			setupError = getErrorMessage(error, 'Could not set a password for this account.');
		} finally {
			setupBusy = false;
		}
	}
</script>

<div class="flex min-h-screen w-full flex-col items-center bg-background px-4 py-10">
	<div class="flex w-full max-w-[420px] flex-col gap-5">
		<div class="flex items-center justify-center gap-2.5">
			<div class="flex size-[26px] flex-none items-center justify-center border border-ink/35">
				<div class="size-2.5 rotate-45 bg-foreground"></div>
			</div>
			<div class="flex flex-col gap-px">
				<div class="text-[15px] font-semibold tracking-[0.26em] text-foreground">SENKEN</div>
				<div class="font-mono text-[8px] tracking-[0.3em] text-dim">RESEARCH TERMINAL</div>
			</div>
		</div>

		<ServerPicker />

		{#if insecure}
			<div
				data-testid="login-insecure-warning"
				class="flex items-start gap-2.5 border border-loss/40 bg-loss/10 px-3 py-2.5"
			>
				<TriangleAlertIcon class="mt-0.5 size-3.5 flex-none text-loss" />
				<p class="font-mono text-[10.5px] leading-relaxed tracking-[0.02em] text-loss">
					INSECURE CONNECTION — {resolveBaseUrl(activeServer())} is neither loopback nor HTTPS.
					Your password will be sent in the clear to anyone on the network path.
				</p>
			</div>
		{/if}

		<div class="border border-ink/16 bg-chrome">
			<Tabs.Root value={tab} onValueChange={(v) => (manualTab = v === 'setup' ? 'setup' : 'login')}>
				<!-- `!bg-foreground`/`!text-inv`/`!shadow-none` on the two triggers below are load-bearing,
				     not decorative emphasis. `ui/tabs/tabs-trigger.svelte`'s own base classes already set
				     `dark:data-[state=active]:bg-input/30` and `dark:data-[state=active]:text-foreground` —
				     a *different* modifier chain (`dark:data-[state=active]:…` vs this file's plain
				     `data-[state=active]:…`), which is exactly the case `ui/dialog/dialog-content.svelte`'s
				     own comment already documents tailwind-merge failing to dedupe ("different modifier
				     chain... the desktop shell is always past the `sm` breakpoint" — same mechanism here,
				     with `dark:` instead of `sm:`). Verified with `getComputedStyle` before adding the `!`:
				     the active tab's background computed to a translucent `bg-input/30` overlay and its
				     text stayed `--fg` (light), not the intended solid `--foreground` background with
				     `--inv` (dark) text — the classes were present in the DOM but doing nothing, the same
				     failure mode as this project's three previously-found dead-class bugs. -->
				<Tabs.List class="w-full rounded-none border-b border-ink/12 bg-transparent p-0">
					<Tabs.Trigger
						value="login"
						class="flex-1 rounded-none border-0 border-r border-ink/12 py-2.5 font-mono text-[10px] tracking-[0.16em] data-[state=active]:!bg-foreground data-[state=active]:!text-inv data-[state=active]:!shadow-none"
					>
						LOG IN
					</Tabs.Trigger>
					<Tabs.Trigger
						value="setup"
						class="flex-1 rounded-none border-0 py-2.5 font-mono text-[10px] tracking-[0.16em] data-[state=active]:!bg-foreground data-[state=active]:!text-inv data-[state=active]:!shadow-none"
					>
						FIRST-TIME SETUP
					</Tabs.Trigger>
				</Tabs.List>

				<Tabs.Content value="login" class="p-4">
					<form class="flex flex-col gap-3" onsubmit={submitLogin}>
						<div class="flex flex-col gap-1.5">
							<Label for="login-email" class="font-mono text-[9px] tracking-[0.16em] text-dim2">EMAIL</Label>
							<Input
								id="login-email"
								type="email"
								autocomplete="username"
								required
								bind:value={loginEmail}
								class="h-9 rounded-none border-ink/16 font-mono text-[12px]"
							/>
						</div>
						<div class="flex flex-col gap-1.5">
							<Label for="login-password" class="font-mono text-[9px] tracking-[0.16em] text-dim2">
								PASSWORD
							</Label>
							<Input
								id="login-password"
								type="password"
								autocomplete="current-password"
								required
								bind:value={loginPassword}
								class="h-9 rounded-none border-ink/16 font-mono text-[12px]"
							/>
						</div>
						{#if loginError}
							<p data-testid="login-error" class="font-mono text-[10.5px] text-loss">{loginError}</p>
						{/if}
						<Button
							type="submit"
							variant="default"
							class="mt-1 h-9 rounded-none font-mono text-[10.5px] tracking-[0.16em]"
							disabled={loginBusy}
						>
							{#if loginBusy}
								<Spinner class="size-3.5" />
							{/if}
							LOG IN
						</Button>
						<p class="text-center font-mono text-[9.5px] tracking-[0.04em] text-dim2">
							First time setting up this server?
							<button
								type="button"
								class="text-foreground underline underline-offset-2"
								onclick={() => (manualTab = 'setup')}
							>
								Set the admin password
							</button>
						</p>
					</form>
				</Tabs.Content>

				<Tabs.Content value="setup" class="p-4">
					<form class="flex flex-col gap-3" onsubmit={submitSetup}>
						<p class="font-mono text-[10px] leading-relaxed tracking-[0.02em] text-dim2">
							This server has a default administrator account with no password set. Choose one below
							to finish setting it up.
						</p>
						<div class="flex flex-col gap-1.5">
							<Label for="setup-email" class="font-mono text-[9px] tracking-[0.16em] text-dim2">
								ADMIN EMAIL
							</Label>
							<Input
								id="setup-email"
								type="email"
								autocomplete="username"
								required
								bind:value={setupEmail}
								class="h-9 rounded-none border-ink/16 font-mono text-[12px]"
							/>
						</div>
						<div class="flex flex-col gap-1.5">
							<Label for="setup-password" class="font-mono text-[9px] tracking-[0.16em] text-dim2">
								NEW PASSWORD
							</Label>
							<Input
								id="setup-password"
								type="password"
								autocomplete="new-password"
								required
								bind:value={setupPassword}
								class="h-9 rounded-none border-ink/16 font-mono text-[12px]"
							/>
							{#if setupTooShort}
								<p class="font-mono text-[9.5px] text-loss">
									At least {MIN_PASSWORD_LENGTH} characters.
								</p>
							{/if}
						</div>
						<div class="flex flex-col gap-1.5">
							<Label for="setup-confirm" class="font-mono text-[9px] tracking-[0.16em] text-dim2">
								CONFIRM PASSWORD
							</Label>
							<Input
								id="setup-confirm"
								type="password"
								autocomplete="new-password"
								required
								bind:value={setupConfirm}
								class="h-9 rounded-none border-ink/16 font-mono text-[12px]"
							/>
							{#if setupMismatch}
								<p class="font-mono text-[9.5px] text-loss">Passwords do not match.</p>
							{/if}
						</div>
						{#if setupError}
							<p data-testid="setup-error" class="font-mono text-[10.5px] text-loss">{setupError}</p>
						{/if}
						<Button
							type="submit"
							variant="default"
							class="mt-1 h-9 rounded-none font-mono text-[10.5px] tracking-[0.16em]"
							disabled={!setupCanSubmit}
						>
							{#if setupBusy}
								<Spinner class="size-3.5" />
							{/if}
							SET PASSWORD & CONTINUE
						</Button>
						<p class="text-center font-mono text-[9.5px] tracking-[0.04em] text-dim2">
							Already set up?
							<button
								type="button"
								class="text-foreground underline underline-offset-2"
								onclick={() => (manualTab = 'login')}
							>
								Log in instead
							</button>
						</p>
					</form>
				</Tabs.Content>
			</Tabs.Root>
		</div>
	</div>
</div>
