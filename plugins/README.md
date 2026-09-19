# Plugins

Everything the running server can load — a market data venue, a trade
adapter, or a dashboard widget UI — lives here, one directory per plugin,
each with a `senken-plugin.json` manifest declaring what it contributes
(`venue`, `trade.adapter`, or `dashboard.widget`) and, for `venue`/
`indicator` packages installed at runtime rather than compiled in, an
`entry` naming its compiled `.wasm` component. Most of what is here today
— every venue and the simulator — is a Rust crate compiled straight into
the server binary, activated unconditionally at startup unless an admin
has disabled it in Settings → Plugins; `widgets/` is the other shape, a
manifest plus a static `web/` bundle installed as a package rather than
compiled in, the same way a venue or an indicator installed at runtime
would be. `book`/`feed` (order-book depth and live streaming) stay static,
compiled-in capabilities for every venue here until they are ported to the
same dynamic, WASM-loaded path bars and instruments already use.
