import tailwindcss from '@tailwindcss/vite';
import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	server: {
		// In dev the UI is served by Vite (hot reload), while `/api` still
		// belongs to the Rust server — so proxy it across. Without this the
		// page would fetch `/api/health` from Vite and get its SPA fallback
		// HTML instead of JSON. Port 4190 is what the root `dev` script
		// starts `senken serve` on; change both together.
		proxy: {
			'/api': {
				target: 'http://127.0.0.1:4190',
				changeOrigin: true
			},
			// A widget plugin's own static files live outside `/api` (see
			// `crates/api/src/lib.rs`'s `mount_widget_plugin_routes` doc
			// comment) so this route can eventually move to a genuinely
			// separate origin. Without proxying it too, the sandboxed iframe's
			// `GET /widget-plugin-assets/...` hits Vite's own dev server, which
			// has no such route and falls back to the SPA's `index.html` — the
			// widget then renders that fallback page instead of its own bundle.
			'/widget-plugin-assets': {
				target: 'http://127.0.0.1:4190',
				changeOrigin: true
			}
		}
	},
	plugins: [
		tailwindcss(),
		sveltekit({
			compilerOptions: {
				// Force runes mode for the project, except for libraries. Can be removed in svelte 6.
				runes: ({ filename }) => filename.split(/[/\\]/).includes('node_modules') ? undefined : true
			},
			// SPA mode: `index.html` is the fallback for every
			// path axum's SPA handler doesn't recognise as a built asset —
			// see crates/api/src/assets.rs.
			adapter: adapter({ fallback: 'index.html' })
		})
	]
});
