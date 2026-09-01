<script module lang="ts">
	import type { SettingsSearchEntry } from '$lib/state/settings-registry.svelte';

	export const searchIndex: SettingsSearchEntry[] = [
		{
			groupHeading: 'Users',
			rowId: 'users-list',
			rowLabel: 'Users',
			rowDescription: 'Create accounts and enable or disable existing ones.'
		},
		{
			groupHeading: 'Users',
			rowId: 'assign-role',
			rowLabel: 'Assign role',
			rowDescription: 'Give a user one of the roles below.'
		},
		{
			groupHeading: 'Users',
			rowId: 'direct-grant',
			rowLabel: 'Direct grants',
			rowDescription: 'Grant or revoke a single permission on a user directly.'
		},
		{
			groupHeading: 'Roles',
			rowId: 'roles-list',
			rowLabel: 'Roles',
			rowDescription: 'Named sets of grants users can be assigned.'
		},
		{
			groupHeading: 'Roles',
			rowId: 'role-grants',
			rowLabel: 'Grants',
			rowDescription: 'Which actions a role permits on which resources.'
		}
	];
</script>

<script lang="ts">
	// Users & roles (admin-only). Q6 built this section's
	// shape against `senken-acl`'s real `Action`/`Resource`/`Scope` enums
	// (mirrored verbatim from `crates/acl/src/{action,resource,scope}.rs`,
	// not guessed) but shipped every control disabled, because Q4 had not
	// built the HTTP endpoints it needs. Q8 built them
	// (`crates/api/src/admin_handlers.rs`) and Q9.3/Q10.1 closed the
	// headless bypass on every mutation they front — this file is Q10.2:
	// wiring this section to that real, tested server surface.
	//
	// Visibility of this *section itself* (whether it appears in the nav at
	// all) is handled one layer up, in `../settings-nav.svelte` via
	// `../access-visibility.svelte.ts` — purely cosmetic and this
	// file's own request handling below is what actually matters: every
	// call here goes through `apiClient`, which re-checks a real grant on
	// every request regardless of what the nav showed. A `403` here
	// (`ForbiddenError`) is displayed as a message and nothing else —
	// never a sign-out, which is reserved for a `401`.
	import { onMount } from 'svelte';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
	import { Input } from '$lib/components/ui/input/index.js';
	import { Button } from '$lib/components/ui/button/index.js';
	import { Badge } from '$lib/components/ui/badge/index.js';
	import * as NativeSelect from '$lib/components/ui/native-select/index.js';
	import { apiClient } from '$lib/api/client';
	import { getErrorMessage } from '$lib/api/errors';
	import type { UserSummaryDto, RoleSummaryDto, GrantDto } from '$lib/api/types';

	/** Mirrors `crates/acl/src/action.rs::Action` exactly. */
	const ACTIONS = ['View', 'Create', 'Edit', 'Delete', 'Share'] as const;
	/** Mirrors `crates/acl/src/resource.rs::Resource` exactly. */
	const RESOURCES = [
		'ChartWorkspace',
		'ChartLayout',
		'Alert',
		'Strategy',
		'Account',
		'Adapter',
		'User',
		'Role',
		'Indicator',
		'Watchlist',
		'Note'
	] as const;
	/** Mirrors `crates/acl/src/scope.rs::Scope` exactly. */
	const SCOPES = ['Own', 'All'] as const;

	/** One page's worth for this settings section — an admin-facing list,
	 * not the terminal's own data grids, so a single generous page (well
	 * under the server's `MAX_LIMIT` of 200, `crates/api/src/admin_handlers.rs`)
	 * covers every installation this app expects without needing pagination
	 * controls here too. */
	const PAGE_SIZE = 100;

	// --- users -------------------------------------------------------------

	let users = $state<UserSummaryDto[]>([]);
	let usersTotal = $state(0);
	let usersLoading = $state(true);
	let usersError = $state<string | null>(null);

	async function loadUsers() {
		usersLoading = true;
		usersError = null;
		try {
			const page = await apiClient.listUsers(PAGE_SIZE, 0);
			users = page.rows;
			usersTotal = page.total;
		} catch (error) {
			usersError = getErrorMessage(error, 'Could not load users.');
		} finally {
			usersLoading = false;
		}
	}

	let showCreateUser = $state(false);
	let newUserEmail = $state('');
	let newUserDisplayName = $state('');
	let newUserPassword = $state('');
	let creatingUser = $state(false);
	let createUserError = $state<string | null>(null);

	const canCreateUser = $derived(
		newUserEmail.trim().length > 0 && newUserDisplayName.trim().length > 0 && !creatingUser
	);

	async function submitCreateUser(event: SubmitEvent) {
		event.preventDefault();
		if (!canCreateUser) return;
		creatingUser = true;
		createUserError = null;
		try {
			await apiClient.createUser({
				email: newUserEmail.trim(),
				display_name: newUserDisplayName.trim(),
				initial_password: newUserPassword.trim() === '' ? null : newUserPassword
			});
			newUserEmail = '';
			newUserDisplayName = '';
			newUserPassword = '';
			showCreateUser = false;
			await loadUsers();
		} catch (error) {
			createUserError = getErrorMessage(error, 'Could not create user.');
		} finally {
			creatingUser = false;
		}
	}

	// --- roles ---------------------------------------------------------------

	let roles = $state<RoleSummaryDto[]>([]);
	let rolesTotal = $state(0);
	let rolesLoading = $state(true);
	let rolesError = $state<string | null>(null);

	async function loadRoles() {
		rolesLoading = true;
		rolesError = null;
		try {
			const page = await apiClient.listRoles(PAGE_SIZE, 0);
			roles = page.rows;
			rolesTotal = page.total;
		} catch (error) {
			rolesError = getErrorMessage(error, 'Could not load roles.');
		} finally {
			rolesLoading = false;
		}
	}

	let showCreateRole = $state(false);
	let newRoleName = $state('');
	let newRoleDescription = $state('');
	// Which (resource, action) checkboxes are checked, keyed `${resource}:${action}`.
	let newRoleGrants = $state<Set<string>>(new Set());
	// One scope choice per resource row, applied to every checked action in
	// that row — the matrix has no per-cell scope column (its own design),
	// so a row-level choice is the least surprising way to wire `Scope` in
	// without redrawing the whole table.
	let newRoleScopes = $state<Record<string, (typeof SCOPES)[number]>>(
		Object.fromEntries(RESOURCES.map((r) => [r, 'Own' as const]))
	);
	let creatingRole = $state(false);
	let createRoleError = $state<string | null>(null);

	function grantKey(resource: string, action: string): string {
		return `${resource}:${action}`;
	}

	function toggleGrant(resource: string, action: string) {
		const key = grantKey(resource, action);
		const next = new Set(newRoleGrants);
		if (next.has(key)) next.delete(key);
		else next.add(key);
		newRoleGrants = next;
	}

	const canCreateRole = $derived(newRoleName.trim().length > 0 && !creatingRole);

	async function submitCreateRole(event: SubmitEvent) {
		event.preventDefault();
		if (!canCreateRole) return;
		creatingRole = true;
		createRoleError = null;
		try {
			const grants: GrantDto[] = RESOURCES.flatMap((resource) =>
				ACTIONS.filter((action) => newRoleGrants.has(grantKey(resource, action))).map((action) => ({
					action,
					resource,
					scope: newRoleScopes[resource]
				}))
			);
			await apiClient.createRole({
				name: newRoleName.trim(),
				description: newRoleDescription.trim(),
				grants
			});
			newRoleName = '';
			newRoleDescription = '';
			newRoleGrants = new Set();
			showCreateRole = false;
			await loadRoles();
		} catch (error) {
			createRoleError = getErrorMessage(error, 'Could not create role.');
		} finally {
			creatingRole = false;
		}
	}

	// --- assign a role to a user --------------------------------------------

	let assignUserId = $state('');
	let assignRoleId = $state('');
	let assigning = $state(false);
	let assignError = $state<string | null>(null);
	let assignSuccess = $state(false);

	const canAssign = $derived(assignUserId !== '' && assignRoleId !== '' && !assigning);

	async function submitAssign() {
		if (!canAssign) return;
		assigning = true;
		assignError = null;
		assignSuccess = false;
		try {
			await apiClient.assignRole(assignUserId, assignRoleId);
			assignSuccess = true;
		} catch (error) {
			assignError = getErrorMessage(error, 'Could not assign that role.');
		} finally {
			assigning = false;
		}
	}

	// --- grant / revoke a direct permission on a user -----------------------

	let directUserId = $state('');
	let directAction = $state<(typeof ACTIONS)[number]>('View');
	let directResource = $state<(typeof RESOURCES)[number]>('ChartWorkspace');
	let directScope = $state<(typeof SCOPES)[number]>('Own');
	let directBusy = $state(false);
	let directError = $state<string | null>(null);
	let directMessage = $state<string | null>(null);

	const canDirect = $derived(directUserId !== '' && !directBusy);

	async function submitGrantDirect() {
		if (!canDirect) return;
		directBusy = true;
		directError = null;
		directMessage = null;
		try {
			await apiClient.grantDirect(directUserId, { action: directAction, resource: directResource, scope: directScope });
			directMessage = 'Granted.';
		} catch (error) {
			directError = getErrorMessage(error, 'Could not grant that permission.');
		} finally {
			directBusy = false;
		}
	}

	async function submitRevokeDirect() {
		if (!canDirect) return;
		directBusy = true;
		directError = null;
		directMessage = null;
		try {
			// `IdentityStore::revoke_direct` matches by `(user, action,
			// resource)` only — `scope` has nothing to disambiguate, since at
			// most one grant per `(user, action, resource)` can exist at all
			// (`crates/identity/src/store.rs`) — but `GrantDto` always
			// carries one, so the field is sent regardless.
			await apiClient.revokeDirect(directUserId, { action: directAction, resource: directResource, scope: directScope });
			directMessage = 'Revoked.';
		} catch (error) {
			directError = getErrorMessage(error, 'Could not revoke that permission.');
		} finally {
			directBusy = false;
		}
	}

	onMount(() => {
		void loadUsers();
		void loadRoles();
	});
</script>

<div class="flex flex-col gap-6">
	<section class="flex flex-col">
		<header class="mb-1 flex items-center justify-between gap-3">
			<div>
				<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">Users</h3>
				<p class="mt-0.5 text-[12px] text-dim2">Create accounts and enable or disable existing ones.</p>
			</div>
			<Button variant="outline" size="sm" onclick={() => (showCreateUser = !showCreateUser)}>
				<PlusIcon class="size-3.5" />
				Create user
			</Button>
		</header>

		{#if showCreateUser}
			<form onsubmit={submitCreateUser} class="mb-2 flex flex-col gap-2 border border-ink/14 bg-card2 p-3">
				<div class="flex flex-wrap gap-2">
					<Input placeholder="Email" bind:value={newUserEmail} class="w-[220px]" />
					<Input placeholder="Display name" bind:value={newUserDisplayName} class="w-[180px]" />
					<Input
						type="password"
						placeholder="Initial password (optional)"
						autocomplete="new-password"
						bind:value={newUserPassword}
						class="w-[200px]"
					/>
					<Button type="submit" size="sm" disabled={!canCreateUser}>
						{creatingUser ? 'Creating…' : 'Create'}
					</Button>
				</div>
				<p class="text-[11px] text-dim">
					Leaving the password blank creates the account behind the same first-run fence the
					default admin uses — the new user sets their own password on first use.
				</p>
				{#if createUserError}
					<p class="text-[11.5px] text-destructive">{createUserError}</p>
				{/if}
			</form>
		{/if}

		{#if usersLoading}
			<p class="text-[12px] text-dim2">Loading users…</p>
		{:else if usersError}
			<div class="flex items-start gap-2 border border-destructive/30 bg-destructive/10 px-3 py-2.5 text-destructive">
				<TriangleAlertIcon class="mt-0.5 size-3.5 flex-none" />
				<p class="text-[12px]">{usersError}</p>
			</div>
		{:else}
			<div class="overflow-x-auto border border-ink/14 bg-card2">
				<table class="w-full min-w-[520px] border-collapse text-left">
					<thead>
						<tr>
							<th class="px-3 py-1.5 text-[10px] font-semibold tracking-[0.08em] text-dim2 uppercase">Email</th>
							<th class="px-3 py-1.5 text-[10px] font-semibold tracking-[0.08em] text-dim2 uppercase">Display name</th>
							<th class="px-3 py-1.5 text-[10px] font-semibold tracking-[0.08em] text-dim2 uppercase">Status</th>
						</tr>
					</thead>
					<tbody>
						{#each users as user (user.id)}
							<tr class="border-t border-ink/9">
								<td class="px-3 py-1.5 font-mono text-[11px] text-secondary-foreground">{user.email}</td>
								<td class="px-3 py-1.5 text-[11.5px] text-foreground">{user.display_name}</td>
								<td class="px-3 py-1.5">
									{#if user.disabled}
										<Badge variant="destructive">Disabled</Badge>
									{:else if !user.password_set}
										<Badge variant="outline">Password not set</Badge>
									{:else}
										<Badge variant="secondary">Active</Badge>
									{/if}
								</td>
							</tr>
						{/each}
						{#if users.length === 0}
							<tr><td colspan="3" class="px-3 py-2.5 text-[11.5px] text-dim">No users.</td></tr>
						{/if}
					</tbody>
				</table>
			</div>
			<p class="mt-1 text-[11px] text-dim">
				Showing {users.length} of {usersTotal}{usersTotal > users.length ? ' (first page only)' : ''}.
			</p>
		{/if}
	</section>

	<section class="flex flex-col">
		<header class="mb-1">
			<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">Assign a role</h3>
			<p class="mt-0.5 text-[12px] text-dim2">Give a user one of the roles below.</p>
		</header>
		<div class="flex flex-wrap items-center gap-2 border border-ink/14 bg-card2 p-3">
			<NativeSelect.Root bind:value={assignUserId} class="w-[220px]">
				<NativeSelect.Option value="">Select a user…</NativeSelect.Option>
				{#each users as user (user.id)}
					<NativeSelect.Option value={user.id}>{user.email}</NativeSelect.Option>
				{/each}
			</NativeSelect.Root>
			<NativeSelect.Root bind:value={assignRoleId} class="w-[200px]">
				<NativeSelect.Option value="">Select a role…</NativeSelect.Option>
				{#each roles as role (role.id)}
					<NativeSelect.Option value={role.id}>{role.name}</NativeSelect.Option>
				{/each}
			</NativeSelect.Root>
			<Button size="sm" onclick={submitAssign} disabled={!canAssign}>
				{assigning ? 'Assigning…' : 'Assign'}
			</Button>
		</div>
		{#if assignError}
			<p class="mt-1 text-[11.5px] text-destructive">{assignError}</p>
		{:else if assignSuccess}
			<p class="mt-1 flex items-center gap-1.5 text-[11.5px] text-gain">
				<CircleCheckIcon class="size-3.5" /> Assigned. The user's other sessions were signed out — their next sign-in will carry the new role.
			</p>
		{/if}
	</section>

	<section class="flex flex-col">
		<header class="mb-1">
			<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">Direct grants</h3>
			<p class="mt-0.5 text-[12px] text-dim2">
				Grant or revoke a single (action, resource, scope) permission on a user directly, independent
				of any role.
			</p>
		</header>
		<div class="flex flex-wrap items-center gap-2 border border-ink/14 bg-card2 p-3">
			<NativeSelect.Root bind:value={directUserId} class="w-[220px]">
				<NativeSelect.Option value="">Select a user…</NativeSelect.Option>
				{#each users as user (user.id)}
					<NativeSelect.Option value={user.id}>{user.email}</NativeSelect.Option>
				{/each}
			</NativeSelect.Root>
			<NativeSelect.Root bind:value={directAction} class="w-[110px]">
				{#each ACTIONS as action (action)}
					<NativeSelect.Option value={action}>{action}</NativeSelect.Option>
				{/each}
			</NativeSelect.Root>
			<NativeSelect.Root bind:value={directResource} class="w-[120px]">
				{#each RESOURCES as resource (resource)}
					<NativeSelect.Option value={resource}>{resource}</NativeSelect.Option>
				{/each}
			</NativeSelect.Root>
			<NativeSelect.Root bind:value={directScope} class="w-[90px]">
				{#each SCOPES as scope (scope)}
					<NativeSelect.Option value={scope}>{scope}</NativeSelect.Option>
				{/each}
			</NativeSelect.Root>
			<Button size="sm" onclick={submitGrantDirect} disabled={!canDirect}>
				{directBusy ? 'Working…' : 'Grant'}
			</Button>
			<Button variant="outline" size="sm" onclick={submitRevokeDirect} disabled={!canDirect}>
				{directBusy ? 'Working…' : 'Revoke'}
			</Button>
		</div>
		{#if directError}
			<p class="mt-1 text-[11.5px] text-destructive">{directError}</p>
		{:else if directMessage}
			<p class="mt-1 flex items-center gap-1.5 text-[11.5px] text-gain">
				<CircleCheckIcon class="size-3.5" />
				{directMessage} The user's other sessions were signed out.
			</p>
		{/if}
	</section>

	<section class="flex flex-col">
		<header class="mb-1 flex items-center justify-between gap-3">
			<div>
				<h3 class="text-[11px] font-semibold tracking-[0.08em] text-foreground uppercase">Roles</h3>
				<p class="mt-0.5 text-[12px] text-dim2">
					Named sets of grants — a role is a set of (action, resource, scope)
					triples, never a free-text permission.
				</p>
			</div>
			<Button variant="outline" size="sm" onclick={() => (showCreateRole = !showCreateRole)}>
				<PlusIcon class="size-3.5" />
				Create role
			</Button>
		</header>

		{#if showCreateRole}
			<form onsubmit={submitCreateRole} class="mb-2 flex flex-col gap-3 border border-ink/14 bg-card2 p-3">
				<div class="flex flex-wrap gap-2">
					<Input placeholder="Role name, e.g. Charts Only" bind:value={newRoleName} class="w-[240px]" />
					<Input placeholder="Description (optional)" bind:value={newRoleDescription} class="w-[280px]" />
				</div>
				<div class="overflow-x-auto">
					<table class="w-full min-w-[640px] border-collapse text-left">
						<thead>
							<tr>
								<th class="w-[110px] pb-1.5 pr-2 text-[10px] font-semibold tracking-[0.08em] text-dim2 uppercase"
									>Resource</th
								>
								<th class="w-[90px] pb-1.5 pr-2 text-[10px] font-semibold tracking-[0.08em] text-dim2 uppercase"
									>Scope</th
								>
								{#each ACTIONS as action (action)}
									<th class="pb-1.5 px-1.5 text-center text-[10px] font-semibold tracking-[0.08em] text-dim2 uppercase">
										{action}
									</th>
								{/each}
							</tr>
						</thead>
						<tbody>
							{#each RESOURCES as resource (resource)}
								<tr class="border-t border-ink/9">
									<td class="py-1.5 pr-2 font-mono text-[11px] text-secondary-foreground">{resource}</td>
									<td class="py-1.5 pr-2">
										<NativeSelect.Root bind:value={newRoleScopes[resource]} size="sm" class="w-[80px]">
											{#each SCOPES as scope (scope)}
												<NativeSelect.Option value={scope}>{scope}</NativeSelect.Option>
											{/each}
										</NativeSelect.Root>
									</td>
									{#each ACTIONS as action (action)}
										<td class="py-1.5 px-1.5 text-center">
											<input
												type="checkbox"
												checked={newRoleGrants.has(grantKey(resource, action))}
												onchange={() => toggleGrant(resource, action)}
												aria-label={`Grant ${action} on ${resource}`}
												class="size-3.5 cursor-pointer"
											/>
										</td>
									{/each}
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
				<div class="flex items-center gap-2">
					<Button type="submit" size="sm" disabled={!canCreateRole}>
						{creatingRole ? 'Creating…' : 'Create role'}
					</Button>
				</div>
				{#if createRoleError}
					<p class="text-[11.5px] text-destructive">{createRoleError}</p>
				{/if}
			</form>
		{/if}

		{#if rolesLoading}
			<p class="text-[12px] text-dim2">Loading roles…</p>
		{:else if rolesError}
			<div class="flex items-start gap-2 border border-destructive/30 bg-destructive/10 px-3 py-2.5 text-destructive">
				<TriangleAlertIcon class="mt-0.5 size-3.5 flex-none" />
				<p class="text-[12px]">{rolesError}</p>
			</div>
		{:else}
			<div class="flex flex-col gap-2">
				{#each roles as role (role.id)}
					<div class="flex flex-col gap-1.5 border border-ink/14 bg-card2 p-3">
						<div class="flex items-center gap-2">
							<span class="text-[12.5px] font-medium text-foreground">{role.name}</span>
							{#if role.builtin}
								<Badge variant="outline">Built in</Badge>
							{/if}
						</div>
						{#if role.description}
							<p class="text-[11.5px] text-dim2">{role.description}</p>
						{/if}
						<div class="flex flex-wrap gap-1">
							{#each role.grants as grant (grant.action + grant.resource + grant.scope)}
								<Badge variant="secondary" class="font-mono">{grant.action} {grant.resource}@{grant.scope}</Badge>
							{/each}
							{#if role.grants.length === 0}
								<span class="text-[11px] text-dim">No grants.</span>
							{/if}
						</div>
					</div>
				{/each}
				{#if roles.length === 0}
					<p class="text-[11.5px] text-dim">No roles.</p>
				{/if}
			</div>
			<p class="mt-1 text-[11px] text-dim">
				Showing {roles.length} of {rolesTotal}{rolesTotal > roles.length ? ' (first page only)' : ''}.
			</p>
		{/if}
	</section>
</div>
