# senken-plugin-host

Loads, runs, and **confines** compiled plugin components.

A plugin is third-party code running inside an application that will hold
broker credentials. The guarantee this crate owes everything else is narrow
and absolute: **nothing a plugin does may take the host down.**

Four mechanisms, and the guarantee fails if any one is missing:

| Failure to prevent | Mechanism |
|---|---|
| A plugin reaching the network or disk | No capabilities granted — not sockets, not HTTP, not the filesystem |
| A plugin exhausting memory from inside the sandbox | A store-level memory ceiling, installed deliberately |
| A runaway loop freezing the application | Wall-clock epoch deadlines while live |
| A backtest that will not reproduce | Deterministic fuel, so the same input always costs the same |

A guest trap returns `Err` to its caller. **No host code may `unwrap()` a
guest result** — that is where the guarantee is kept or lost.

This crate also backs `wit/senken.wit`'s `builtins` import (`src/
builtins.rs`) with the real `senken_indicators` state machines, so a
plugin's `ema(close, 20)` calls the same compiled, already-tested `Ema`
this application uses everywhere else. This is the one place a domain
dependency belongs: `senken-plugin-api` (the published SDK a plugin
compiles against) must never depend on a Senken domain crate, but this
crate — the layer that actually mediates every call a plugin makes back
into the host — is exactly where that mediation is supposed to live. Every
built-in's state is kept on the plugin instance's own `Store`, never on
the shared `Linker`, so one plugin's indicator state never leaks into
another's.

Two more pieces sit on top of those four, for the same reason:

- **A circuit breaker per plugin.** Repeated traps disable a plugin for a
  cooldown instead of calling into it again on every bar — a plugin that
  cannot run correctly should not be retried at full speed forever, and the
  breaker's reason string says why it opened.
- **A bounded, per-plugin log**, not a file. A broken plugin can print on
  every call it makes; a ring buffer keeps that history around without
  letting a runaway plugin exhaust the disk the same way it was already
  denied a socket to reach out through.
