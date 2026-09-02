<script lang="ts">
	// The sandboxed iframe host for one placed instance of a dynamic widget
	// UI package. This is the "surface" half of the two-tier UI
	// contribution model: the plugin ships code that draws itself, so it
	// gets an iframe; a toolbar item or menu entry would instead be pure
	// data the host renders with its own components, no iframe at all.
	//
	// Two things are checked on *every* inbound message before a single
	// field of it is trusted, because nothing about who sent it is implied
	// by the fact a `message` event fired at all:
	//
	// - `event.source` must be this exact iframe's own `contentWindow` —
	//   never some other frame, another widget's iframe, or a popup.
	// - `event.origin` must be the literal string `"null"`. A sandboxed
	//   iframe with `sandbox="allow-scripts"` and no `allow-same-origin`
	//   always gets an **opaque** origin, regardless of what URL actually
	//   served its document — so this check holds even though the widget's
	//   bundle happens to be served from this same host today. This is the
	//   platform's real isolation boundary, not which origin served the
	//   file (see `crates/api/src/widget_plugin_handlers.rs`'s own note on
	//   this).
	//
	// A message that passes both checks is then checked for shape —
	// `channel`/`v`/`id`/`method` all present and of the expected type —
	// before its `method` is dispatched at all. Anything that fails either
	// check is dropped silently; a widget's own bug or a hostile page
	// embedding something unexpected must never be able to reach
	// `config.patch` by accident.
	//
	// The mockup label ("this is fixture data") is drawn by
	// `widget-frame.svelte` from the *catalog's* `data_source`, which this
	// component never touches and the widget inside the iframe can never
	// see or influence — a widget cannot lie about its own mockup status
	// because it is never asked.
	import { onDestroy, onMount } from 'svelte';
	import { isFromSandboxedWidget, isWidgetToHostMessage } from './widget-message-protocol';

	let {
		entryUrl,
		widgetTypeId,
		config,
		onConfigChange
	}: {
		/** This server's own URL serving the widget's entry document. */
		entryUrl: string;
		/** `<provider_id>/<widget id>` — handed to the widget on `ready` so
		 * a bundle serving more than one widget type can tell which one it
		 * is running as. */
		widgetTypeId: string;
		/** This placed instance's own config, as opaque JSON-object text —
		 * exactly `senken_dashboard::WidgetRecord::config`'s shape. */
		config: string;
		/** Called with the new config text once the widget successfully
		 * patches its own config through `config.patch`. The caller is
		 * responsible for persisting it (the same debounced save every
		 * other layout edit already goes through). */
		onConfigChange?: (config: string) => void;
	} = $props();

	let iframeEl = $state<HTMLIFrameElement | undefined>();

	function parseConfig(raw: string): Record<string, unknown> {
		try {
			const parsed: unknown = JSON.parse(raw);
			return parsed && typeof parsed === 'object' && !Array.isArray(parsed)
				? (parsed as Record<string, unknown>)
				: {};
		} catch {
			return {};
		}
	}

	// The widget's own working copy of its config. Re-parsed only when a
	// genuinely different instance mounts (the `{#key}` in
	// `dashboard-grid.svelte`'s render loop remounts this component on
	// widget id change, so `$state`'s own initializer running once per
	// mount is exactly right here — reading `config` again on every prop
	// change would overwrite a patch this same component just applied
	// locally with whatever the caller passed in last).
	let currentConfig = $state(parseConfig(config));

	function reply(id: string, result: unknown): void {
		const target = iframeEl?.contentWindow;
		if (!target) return;
		// The target origin is `'*'`, deliberately: the iframe's own
		// origin is opaque (no `allow-same-origin`), so there is no real
		// origin string to name on the way out either — the boundary is
		// the sandbox itself, plus the checks `onMessage` already applied
		// on the way in.
		target.postMessage({ channel: 'senken.widget', v: 1, id, ok: true, result }, '*');
	}

	function onMessage(event: MessageEvent): void {
		if (!isFromSandboxedWidget(event, iframeEl)) return;
		if (!isWidgetToHostMessage(event.data)) return;

		const { id, method, params } = event.data;
		switch (method) {
			case 'ready':
			case 'config.get':
				reply(id, { widgetTypeId, config: currentConfig });
				break;
			case 'config.patch': {
				const patch = params?.patch;
				if (patch && typeof patch === 'object' && !Array.isArray(patch)) {
					currentConfig = { ...currentConfig, ...(patch as Record<string, unknown>) };
					onConfigChange?.(JSON.stringify(currentConfig));
				}
				reply(id, { widgetTypeId, config: currentConfig });
				break;
			}
		}
	}

	onMount(() => {
		window.addEventListener('message', onMessage);
	});
	onDestroy(() => {
		window.removeEventListener('message', onMessage);
	});
</script>

<iframe
	bind:this={iframeEl}
	src={entryUrl}
	title={widgetTypeId}
	sandbox="allow-scripts"
	class="size-full border-0"
	data-widget-plugin-iframe
	data-widget-type-id={widgetTypeId}
></iframe>
