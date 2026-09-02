import { redirect } from '@sveltejs/kit';

// The landing route is the dashboard, and the dashboard that counts is the
// server-backed one: workspaces a user owns, saved and reloaded, not the
// local-only fixture page this route used to render. Redirecting rather than
// moving the page keeps every existing bookmark and deep link working.
export const load = () => {
	redirect(307, '/dashboard');
};
