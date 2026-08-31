<script lang="ts">
	// 52px nav sidebar: the three NAV
	// entries, a spacer, the theme toggle and a static settings
	// icon. It sits below TopBar, not inside it — the reference's markup
	// nests both under one flex column, but the theme toggle visually and
	// structurally belongs to this rail, not the 46px header strip above it.
	import { page } from '$app/state';
	import { cn } from '$lib/utils.js';
	import { toggleMode } from 'mode-watcher';
	import { NAV } from '$lib/mock/shell';
	import * as Tooltip from '$lib/components/ui/tooltip/index.js';
	import MoonIcon from '@lucide/svelte/icons/moon';
	import SunIcon from '@lucide/svelte/icons/sun';
	import SettingsIcon from '@lucide/svelte/icons/settings';
	// the settings modal's trigger lives here — this rail owns
	// the gear icon, and the modal itself is a self-contained overlay (like
	// `layout/command-palette.svelte`, but mounted from its own trigger
	// rather than from `app-shell.svelte`, which owns the
	// auth gate). Mounting it here keeps every render of the trigger paired
	// with exactly one modal instance, with no dependency on a file this
	// milestone does not own.
	import SettingsModal from '$lib/components/settings/settings-modal.svelte';
	import { openSettings } from '$lib/state/settings.svelte';

	const flyoutClass =
		'rounded-none border border-ink/16 bg-popover px-3 py-2.5 text-left shadow-[0_14px_34px_rgba(0,0,0,0.65)]';
</script>

<Tooltip.Provider>
	<nav
		class="relative z-40 flex w-[52px] flex-none flex-col items-center gap-0.5 border-r border-border bg-chrome py-2"
	>
		{#each NAV as n (n.key)}
			{@const active = page.url.pathname === n.href}
			<Tooltip.Root>
				<Tooltip.Trigger>
					{#snippet child({ props })}
						<a
							{...props}
							href={n.href}
							aria-current={active ? 'page' : undefined}
							aria-label={n.label}
							class={cn(
								'flex h-[34px] w-9 items-center justify-center border transition-colors',
								active
									? 'border-foreground bg-foreground text-inv'
									: 'border-transparent text-dim2 hover:bg-ink/7 hover:text-foreground'
							)}
						>
							<n.icon class="size-4" />
						</a>
					{/snippet}
				</Tooltip.Trigger>
				<Tooltip.Content side="right" sideOffset={14} arrowClasses="hidden" class={flyoutClass}>
					<span class="text-[11.5px] font-semibold whitespace-nowrap tracking-[0.16em] text-foreground">
						{n.label}
					</span>
				</Tooltip.Content>
			</Tooltip.Root>
		{/each}

		<div class="flex-1"></div>

		<Tooltip.Root>
			<Tooltip.Trigger
				class="flex h-[34px] w-9 cursor-pointer items-center justify-center text-secondary-foreground"
				onclick={toggleMode}
				aria-label="Toggle theme"
			>
				<SunIcon class="size-4 dark:hidden" />
				<MoonIcon class="hidden size-4 dark:block" />
			</Tooltip.Trigger>
			<Tooltip.Content side="right" sideOffset={18} arrowClasses="hidden" class={flyoutClass}>
				<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">
					<span class="dark:hidden">SWITCH TO DARK</span>
					<span class="hidden dark:inline">SWITCH TO LIGHT</span>
				</span>
			</Tooltip.Content>
		</Tooltip.Root>

		<Tooltip.Root>
			<Tooltip.Trigger
				class="flex h-[34px] w-9 cursor-pointer items-center justify-center text-dim hover:text-foreground"
				onclick={() => openSettings()}
				aria-label="Settings"
			>
				<SettingsIcon class="size-4" />
			</Tooltip.Trigger>
			<Tooltip.Content side="right" sideOffset={18} arrowClasses="hidden" class={flyoutClass}>
				<span class="font-mono text-[9.5px] tracking-[0.16em] whitespace-nowrap text-secondary-foreground">
					SETTINGS
				</span>
			</Tooltip.Content>
		</Tooltip.Root>
	</nav>
</Tooltip.Provider>

<SettingsModal />
