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
	import { isDesktopShell } from '$lib/shell';
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
	 * Whether this server's default administrator still has no password, and
	 * so needs setting up rather than logging into.
	 *
	 * `GET /api/health`'s `needs_setup` reports that account's real state,
	 * unauthenticated, without accepting or checking a password — no
	 * account-enumeration surface, because it always names the one fixed,
	 * already-public default admin rather than a caller-supplied account.
	 *
	 * `null` means the answer has not arrived: either the request is still
	 * in flight, or it failed and never will. Those need different
	 * treatment, so `healthChecked` below distinguishes them — showing the
	 * log-in form while the answer is still coming would flip it to the
	 * setup form a moment later on a fresh install, which reads as the page
	 * breaking rather than as it loading.
	 */
	let needsSetup = $state<boolean | null>(null);
	/** `true` once the health request has settled, either way. An
	 * unreachable server falls back to the log-in form: the setup form
	 * cannot be the safer default, since offering to set a password on a
	 * server that already has one is the more alarming of the two guesses. */
	let healthChecked = $state(false);
	let lastServerId = activeServer().id;

	/** Fetches `needsSetup` for `serverId` from the server itself. `GET
	 * /api/health` needs no credential, so this can run before any session
	 * exists at all. */
	async function loadNeedsSetup(serverId: string): Promise<void> {
		try {
			const health = await apiClient.health();
			if (activeServer().id !== serverId) return; // superseded by a server switch
			needsSetup = health.needs_setup;
		} catch {
			// Leave `needsSetup` as `null` — see its own doc above.
		} finally {
			if (activeServer().id === serverId) healthChecked = true;
		}
	}

	$effect(() => {
		const id = activeServer().id;
		if (id !== lastServerId) {
			lastServerId = id;
			needsSetup = null;
			healthChecked = false;
		}
		void loadNeedsSetup(id);
	});

	/** Exactly one of the two forms, never a choice between them. Which one
	 * is a fact about the server, not a preference: an account that has a
	 * password cannot be set up again (the server refuses it), and one that
	 * has none cannot be logged into. Offering both as tabs asked the user
	 * to guess something the server already knows. */
	const form = $derived(needsSetup === true ? 'setup' : 'login');

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

		<!-- Only the desktop app can point itself at a different server. A
		     browser tab was served *by* one, and switching would mean navigating
		     away from the page doing the switching. -->
		{#if isDesktopShell()}
			<ServerPicker />
		{/if}

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
			{#if healthChecked}
				<div class="border-b border-ink/12 px-4 py-2.5">
					<span class="font-mono text-[10px] tracking-[0.16em] text-foreground">
						{form === 'setup' ? 'FIRST-TIME SETUP' : 'LOG IN'}
					</span>
				</div>
			{/if}

			{#if !healthChecked}
				<!-- Which form belongs here is the server's answer, not a guess.
				     Rendering one and swapping it a moment later reads as the page
				     breaking, so nothing is offered until the answer arrives. -->
				<div class="flex items-center justify-center gap-2 p-8">
					<Spinner class="size-3.5" />
					<span class="font-mono text-[10px] tracking-[0.16em] text-dim2">CHECKING SERVER…</span>
				</div>
			{:else if form === 'setup'}
				<div class="p-4">
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
					</form>
				</div>
			{:else}
				<div class="p-4">
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
					</form>
				</div>
			{/if}
		</div>
	</div>
</div>
