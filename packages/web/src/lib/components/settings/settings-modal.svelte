<script lang="ts">
	// The settings modal itself: "a near-fullscreen modal,
	// not a route — it opens over whatever you were doing and returns you
	// there." Structure mirrors the reference (Zed's settings window) and
	// this app's own existing large-overlay pattern
	// (`layout/command-palette.svelte`): a bits-ui `Dialog` sized to the
	// viewport rather than a fixed pixel width, `border` (not just a
	// border *color* — see that file's header comment on the
	// `border-ink/18`-with-no-width bug this app already found once) on
	// `Dialog.Content`, and `bg-popover` (not `bg-pop`, which this app's
	// own `@theme inline` block never exports — see `layout.css`'s comment
	// on that dead token) for the panel surface.
	//
	// Importing `register-core-sections` here (for its side effect) is
	// what makes Core's five sections exist at all — see that file's
	// header comment for why this is not a special, privileged
	// registration path.
	import './register-core-sections';
	import { tick } from 'svelte';
	import * as Dialog from '$lib/components/ui/dialog/index.js';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import SettingsNav from './settings-nav.svelte';
	import {
		listSettingsSections,
		isSettingsSectionVisible
	} from '$lib/state/settings-registry.svelte';
	import { isDesktopShell } from '$lib/shell';
	import { accessVisibility, canSeeAccessSection } from './access-visibility.svelte';
	import { settingsModal, closeSettings } from '$lib/state/settings.svelte';
	import XIcon from '@lucide/svelte/icons/x';

	let navRef = $state<{ focusSearch: () => void } | null>(null);
	let scrollEl = $state<HTMLDivElement | null>(null);

	// The same list the nav shows. Resolving the active section against the
	// unfiltered registry would render a section the nav does not list —
	// reachable by a stale `activeSectionId` from a previous session.
	const sections = $derived(
		listSettingsSections().filter((section) =>
			isSettingsSectionVisible(section, {
				isAdmin: canSeeAccessSection(accessVisibility.profile),
				isDesktop: isDesktopShell()
			})
		)
	);
	const activeSection = $derived(
		sections.find((s) => s.id === settingsModal.activeSectionId) ?? sections[0] ?? null
	);

	// No section was picked yet (first open) or the picked id no longer
	// resolves (e.g. a plugin section that unregistered) — land on the
	// first section by `order` rather than showing a blank pane.
	$effect(() => {
		if (settingsModal.open && activeSection && settingsModal.activeSectionId !== activeSection.id) {
			settingsModal.activeSectionId = activeSection.id;
		}
	});

	// Cross-section search jump: scroll the target row into view and
	// flash it, then clear the pending id so re-picking the same result
	// later still triggers the effect.
	$effect(() => {
		const rowId = settingsModal.pendingHighlightRowId;
		if (!rowId || !scrollEl) return;
		void tick().then(() => {
			const el = scrollEl?.querySelector(`[data-settings-row="${CSS.escape(rowId)}"]`);
			if (el instanceof HTMLElement) {
				el.scrollIntoView({ block: 'center', behavior: 'smooth' });
				el.classList.add('settings-row-flash');
				setTimeout(() => el.classList.remove('settings-row-flash'), 1200);
			}
			settingsModal.pendingHighlightRowId = null;
		});
	});
</script>

<Dialog.Root
	bind:open={settingsModal.open}
	onOpenChange={(v) => {
		if (!v) closeSettings();
	}}
>
	<Dialog.Content
		showCloseButton={false}
		onOpenAutoFocus={(e) => {
			e.preventDefault();
			navRef?.focusSearch();
		}}
		class="top-1/2 left-1/2 flex h-[min(840px,92vh)] w-[min(1120px,94vw)] max-w-none -translate-x-1/2 -translate-y-1/2 flex-col gap-0 overflow-hidden border border-ink/18 bg-popover p-0 shadow-[0_32px_80px_rgba(0,0,0,0.75)] ring-0"
	>
		<Dialog.Header class="sr-only">
			<Dialog.Title>Settings</Dialog.Title>
			<Dialog.Description>App, account and connection settings</Dialog.Description>
		</Dialog.Header>

		<!-- One Provider for the whole panel: the header's own close-button
		     tooltip and every row-level reset tooltip a section renders
		     (`settings-row.svelte`) share it, rather than each needing its
		     own — bits-ui's `Tooltip.Root` reads shared config from the
		     nearest `Tooltip.Provider` ancestor, the same relationship
		     `nav-rail.svelte` already relies on for its own tooltips. -->
		<Tooltip.Provider>
			<div class="flex flex-none items-center justify-between border-b border-ink/10 px-4 py-[13px]">
				<span class="font-mono text-[12px] font-medium tracking-[0.12em] text-foreground">SETTINGS</span>
				<Tooltip.Root>
					<Tooltip.Trigger>
						{#snippet child({ props })}
							<button
								{...props}
								type="button"
								onclick={closeSettings}
								aria-label="Close settings"
								class="flex size-6 cursor-pointer items-center justify-center text-dim2 hover:text-foreground"
							>
								<XIcon class="size-4" />
							</button>
						{/snippet}
					</Tooltip.Trigger>
					<Tooltip.Content side="left" class="font-mono text-[9.5px] tracking-[0.1em]">ESC</Tooltip.Content>
				</Tooltip.Root>
			</div>

			<div class="flex min-h-0 flex-1">
				<SettingsNav bind:this={navRef} />

				<div bind:this={scrollEl} class="min-w-0 flex-1 overflow-y-auto px-6 py-5">
					{#if activeSection}
						<h2 class="mb-4 text-[15px] font-semibold text-foreground">{activeSection.label}</h2>
						<activeSection.component />
					{:else}
						<p class="text-[12.5px] text-dim2">No settings sections are registered.</p>
					{/if}
				</div>
			</div>
		</Tooltip.Provider>
	</Dialog.Content>
</Dialog.Root>

<style>
	:global(.settings-row-flash) {
		animation: settings-row-flash 1.2s ease-out;
	}
	@keyframes settings-row-flash {
		0% {
			background-color: rgba(var(--ink), 0.14);
		}
		100% {
			background-color: transparent;
		}
	}
</style>
