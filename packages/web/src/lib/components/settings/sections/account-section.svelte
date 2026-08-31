<script module lang="ts">
	import type { SettingsSearchEntry } from '$lib/state/settings-registry.svelte';

	export const searchIndex: SettingsSearchEntry[] = [
		{
			groupHeading: 'Profile',
			rowId: 'profile-email',
			rowLabel: 'Email',
			rowDescription: 'The address this account signs in with.'
		},
		{
			groupHeading: 'Security',
			rowId: 'change-password',
			rowLabel: 'Password',
			rowDescription: 'Change the password used to sign in.'
		},
		{
			groupHeading: 'Security',
			rowId: 'log-out',
			rowLabel: 'Sign out',
			rowDescription: 'End this session on this device.'
		}
	];
</script>

<script lang="ts">
	// Account section. Backed entirely by the auth endpoints
	// Q4 actually shipped: `GET /api/me` for the profile, `POST
	// /api/set-password` for the change-password row, `POST /api/logout`
	// for signing out. There is no "edit display name" endpoint anywhere in
	// `crates/api` (only `email`/`display_name`/`disabled`/`password_set`
	// are *read* by `MeResponse` — nothing writes them beyond the seed
	// migration and the password itself), so this section shows the
	// profile fields read-only rather than rendering editable inputs that
	// would silently do nothing on submit.
	import { onMount } from 'svelte';
	import SettingsGroup from '../settings-group.svelte';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Input } from '$lib/components/ui/input/index.js';
	import { ForbiddenError, UnauthorizedError } from '$lib/api/errors';
	import { activeServer } from '$lib/api/servers.svelte';
	import { endSession } from '$lib/api/session.svelte';
	import { setConnectionState } from '$lib/api/connection.svelte';
	import { apiClient } from '$lib/api/client';
	import { MIN_PASSWORD_LENGTH } from '$lib/api/constants';
	import type { MeResponse } from '$lib/api/types';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';

	let profile = $state<MeResponse | null>(null);
	let loadError = $state<string | null>(null);
	let loading = $state(true);

	let newPassword = $state('');
	let confirmPassword = $state('');
	let passwordBusy = $state(false);
	let passwordError = $state<string | null>(null);
	let passwordSuccess = $state(false);

	let logoutBusy = $state(false);
	let logoutError = $state<string | null>(null);

	async function load() {
		loading = true;
		loadError = null;
		try {
			profile = await apiClient.me();
		} catch (error) {
			loadError = error instanceof Error ? error.message : 'Could not load your profile.';
		} finally {
			loading = false;
		}
	}

	onMount(load);

	const passwordMismatch = $derived(
		confirmPassword.length > 0 && newPassword !== confirmPassword
	);
	const passwordTooShort = $derived(
		newPassword.length > 0 && newPassword.length < MIN_PASSWORD_LENGTH
	);
	const canSubmitPassword = $derived(
		newPassword.length >= MIN_PASSWORD_LENGTH && newPassword === confirmPassword && !passwordBusy
	);

	async function submitPassword(event: SubmitEvent) {
		event.preventDefault();
		if (!canSubmitPassword) return;
		passwordBusy = true;
		passwordError = null;
		passwordSuccess = false;
		try {
			await apiClient.setPassword(newPassword);
			passwordSuccess = true;
			newPassword = '';
			confirmPassword = '';
		} catch (error) {
			// B16 point 3: a 403 here is genuinely unexpected (changing your
			// own password needs no grant per `senken_identity`'s own doc
			// comment), but handled the same non-logout way regardless of
			// cause.
			passwordError =
				error instanceof ForbiddenError
					? error.message
					: error instanceof Error
						? error.message
						: 'Could not change your password.';
		} finally {
			passwordBusy = false;
		}
	}

	// Explicit sign-out. This drives the same observable state `ApiClient`'s
	// own 401 handling drives for an *involuntary* session loss (`client.ts`'s
	// `handleSessionExpired`): end the session and set
	// `connectionStore.state` to `'disconnected'` — the login route
	// (`routes/login/+page.svelte`) reacts to that same signal via
	// `app-shell.svelte`'s auth gate, so this does not need to `goto`
	// anywhere itself.
	//
	// Calls `apiClient.request` directly (this *is*
	// the funnel) rather than `apiClient.logout()` — that convenience method
	// restarts the heartbeat afterward (right, for its own callers: a
	// session can end for reasons besides a user clicking "sign out"), which
	// would immediately undo the `'disconnected'` state this function exists
	// to produce (`stopHeartbeat` below is required for the same reason:
	// `startHeartbeat`'s poll loop treats a successful `/api/health` as
	// `'authenticated'` *regardless of whether a credential is stored* —
	// verified live, `connectionStore.state` flipped back to `'authenticated'`
	// on the very next tick, ≤5s, without it).
	//
	// Ends the session through `endSession` rather than clearing the
	// credential directly, on both the success path here and the "already
	// logged out" 401 path below — that is what also closes the settings
	// modal this button lives in, so a later login doesn't land back on it.
	async function submitLogout() {
		logoutBusy = true;
		logoutError = null;
		try {
			await apiClient.request<void>('/api/logout', { method: 'POST' });
		} catch (error) {
			if (!(error instanceof UnauthorizedError)) {
				// A network/HTTP failure other than "already logged out" —
				// surface it and leave the session alone rather than
				// clearing a credential the server never actually revoked.
				logoutError = error instanceof Error ? error.message : 'Could not sign out.';
				logoutBusy = false;
				return;
			}
			// Already logged out server-side (a 401) — proceed to clear
			// local state exactly as if the call had succeeded.
		}
		apiClient.stopHeartbeat();
		endSession(activeServer().id);
		setConnectionState('disconnected');
		logoutBusy = false;
	}
</script>

{#snippet emailControl()}
	<span class="font-mono text-[12px] text-secondary-foreground">{profile?.email ?? '—'}</span>
{/snippet}

{#snippet displayNameControl()}
	<span class="font-mono text-[12px] text-secondary-foreground">{profile?.display_name ?? '—'}</span>
{/snippet}

<div class="flex flex-col gap-6">
	{#if loading}
		<p class="text-[12px] text-dim2">Loading your profile…</p>
	{:else if loadError}
		<div class="flex items-start gap-2 border border-destructive/30 bg-destructive/10 px-3 py-2.5 text-destructive">
			<TriangleAlertIcon class="mt-0.5 size-3.5 flex-none" />
			<p class="text-[12px]">{loadError}</p>
		</div>
	{:else}
		<SettingsGroup
			group={{
				id: 'profile',
				heading: 'Profile',
				description: 'Read-only — there is no self-service profile editor yet.',
				rows: [
					{
						id: 'profile-email',
						label: 'Email',
						description: 'The address this account signs in with.',
						control: emailControl
					},
					{
						id: 'profile-display-name',
						label: 'Display name',
						description: 'How your account is shown around the app.',
						control: displayNameControl
					}
				]
			}}
		/>

		<section class="flex flex-col">
			<header class="mb-1">
				<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">Security</h3>
				<p class="mt-0.5 text-[12px] text-dim2">Change your password or end this session.</p>
			</header>
			<div class="flex flex-col gap-4 divide-y divide-border">
				<form onsubmit={submitPassword} class="flex flex-col gap-2.5 py-3">
					<div class="flex items-start justify-between gap-6">
						<div class="flex min-w-0 flex-1 flex-col gap-0.5 pt-0.5">
							<span class="text-[13px] font-medium text-foreground">Password</span>
							<span class="text-[12px] leading-snug text-dim2">
								At least {MIN_PASSWORD_LENGTH} characters. No current-password confirmation is
								required — you are already signed in.
							</span>
						</div>
					</div>
					<div class="flex flex-wrap items-center gap-2">
						<Input
							type="password"
							placeholder="New password"
							autocomplete="new-password"
							bind:value={newPassword}
							class="w-[220px]"
						/>
						<Input
							type="password"
							placeholder="Confirm new password"
							autocomplete="new-password"
							bind:value={confirmPassword}
							class="w-[220px]"
						/>
						<Button type="submit" size="sm" disabled={!canSubmitPassword}>
							{passwordBusy ? 'Saving…' : 'Change password'}
						</Button>
					</div>
					{#if passwordTooShort}
						<p class="text-[11.5px] text-destructive">
							Password must be at least {MIN_PASSWORD_LENGTH} characters.
						</p>
					{:else if passwordMismatch}
						<p class="text-[11.5px] text-destructive">Passwords do not match.</p>
					{:else if passwordError}
						<p class="text-[11.5px] text-destructive">{passwordError}</p>
					{:else if passwordSuccess}
						<p class="flex items-center gap-1.5 text-[11.5px] text-gain">
							<CircleCheckIcon class="size-3.5" /> Password changed. Other sessions for this account
							were signed out.
						</p>
					{/if}
				</form>

				<div class="flex items-center justify-between gap-6 pt-3">
					<div class="flex min-w-0 flex-1 flex-col gap-0.5">
						<span class="text-[13px] font-medium text-foreground">Sign out</span>
						<span class="text-[12px] leading-snug text-dim2">End this session on this device.</span>
					</div>
					<Button variant="outline" size="sm" onclick={submitLogout} disabled={logoutBusy}>
						{logoutBusy ? 'Signing out…' : 'Sign out'}
					</Button>
				</div>
				{#if logoutError}
					<p class="text-[11.5px] text-destructive">{logoutError}</p>
				{/if}
			</div>
		</section>
	{/if}
</div>
