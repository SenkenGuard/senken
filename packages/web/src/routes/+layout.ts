// SPA mode: no server-side rendering, no prerendering. The
// whole app is one static bundle served (and, in `senken gui`, embedded)
// by `senken-api`, with client-side routing handled entirely in the
// browser/webview.
export const ssr = false;
export const prerender = false;
