# senken-indicator-lang

The language a trader writes an indicator in, compiled to WebAssembly **inside
the application**, in milliseconds, with no toolchain on anyone's machine.

Compiling Rust on the server would mean shipping a 1–2 GB toolchain or running
a build service. Compiling a small expression language does not — which is the
whole reason this crate exists.

What keeps it small, and what must not be relaxed:

- **No I/O, files, or network — not preventable, but inexpressible.** That is
  the real security layer; the WebAssembly sandbox is the second one.
- **No modules, no user-defined types, no generics.**
- **The standard library is Senken's own built-in indicators**, exposed as
  builtins. `ema(close, 20)` calls compiled, already-tested Rust; user code is
  the glue between such calls.
- **No looping over history.** That is the most direct way to break the
  incremental contract, and if it were available it would be used.

Error messages are a feature, not a courtesy: the authoring panel shows them
verbatim to someone who is not a programmer, so they name a line and a column
and never a compiler-internal term.

## The language

A program is a sequence of `let` bindings followed by exactly one `plot`
line — the indicator's output. Comments run from `//` to end of line.

```
// A MACD histogram, smoothed by a manual difference of two EMAs, plotted
// alongside Bollinger's upper band.
let fast = ema(close, 12)
let slow = ema(close, 26)
plot (fast - slow) + bollinger(20, 2.0).upper
```

- **Bar fields** `open`, `high`, `low`, `close`, `volume` name the current
  bar without a `let` — always the bar `on-bar` was just called with, never
  a value from history.
- **Arithmetic** is `+ - * /` and unary `-`, with the usual precedence and
  parentheses.
- **Built-in calls** are the standard library: `sma`, `ema`, `wma`, `rsi`,
  `atr`, `vwap`, `volume`, `stochastic`, `macd`, `bollinger`. `sma`/`ema`/
  `wma` take an arbitrary numeric expression as their series argument
  (`ema(high - low, 14)` is valid); every other built-in reads the bar
  fields its `senken_indicators` equivalent's own `handle_bar` reads, so it
  takes only its period/constant arguments. A period or a numeric constant
  argument (Bollinger's band width) must be written as a literal directly
  in the call — `ema(close, 20)`, never `ema(close, some_let)` — because
  `senken_indicators`' state for that built-in is constructed once, at
  compile time, from that exact value.
- **A built-in that reports more than one number** (`stochastic` ->
  `k`/`d`; `macd` -> `macd`/`signal`/`histogram`; `bollinger` ->
  `upper`/`middle`/`lower`) must be narrowed with `.field` before it can be
  used in an expression, e.g. `macd(12, 26, 9).histogram`. Naming a wrong
  field, or using the call bare, is a type error that lists the built-in's
  actual field names.
- **A `let` always holds a number.** A multi-valued built-in's result must
  be projected with `.field` in the same expression that produces it —
  `let hist = macd(12, 26, 9).histogram` is fine, `let m = macd(12, 26, 9)`
  is not.

## Why exactly one `plot`

This is this MVP's own choice, not a constraint the language's stated
design forces. Every one of the ten built-ins — including each field of a
multi-valued one — is exercisable and provable equivalent to its
`senken_indicators` counterpart through a single scalar output, which is
what this crate's test suite actually needs. Supporting more than one
`plot` (for a MACD-shaped indicator that wants to draw all three of its own
lines at once) is a straightforward extension — `on-bar` would return a
small tuple instead of one `f64`, the same return-pointer mechanism this
crate already uses for a multi-valued built-in's result — left for when a
caller actually needs it rather than built speculatively.

## The compiled artifact

`compile()` turns source into a component implementing `wit/senken.wit`'s
`compiled-indicator` world — **not** that same file's `indicator-plugin`
world, the one a Rust-authored plugin implements. `indicator-plugin`'s
`indicator` interface carries strings (a descriptor's id and title), lists
(params, plots, drawables) and a resource (`instance`), none of which this
language can produce: it has no way to declare a title or a configurable
parameter. Targeting it directly would mean hand-writing, for every
compiled program, the canonical-ABI glue `wit-bindgen` normally generates
for records, lists, strings and resources — real engineering, but
identical for every compiled program and unrelated to what that program
actually computes.

`compiled-indicator` is the boundary this compiler actually needs: every
built-in update is scalars in, scalars (or a small fixed-size tuple, via a
return pointer into a bump-allocated scratch buffer reset every bar) out,
and the compiled program itself exports one `on-bar: func(open, high, low,
close, volume: f64) -> f64`. It imports the same `builtins` interface
`indicator-plugin` does, so there is exactly one definition, workspace-wide,
of what calling a built-in means. Bridging a compiled program into the
full `indicator-plugin` world — supplying its descriptor, owning its
resource handle, building its `on-bar-result` list from this world's one
scalar — is generic glue, identical for every compiled program, that
belongs with whatever loads a plugin into that world, not with the
compiler that produces it. See `src/codegen/mod.rs` for the full
reasoning, and the workspace's `wit/senken.wit` for the ABI itself.

Every built-in call reuses the real `senken_indicators` type via a host
import (`ema(close, 20)` calls the host's `ema-update`, which calls
`Ema::update_raw` directly) — there is no reimplementation of any
formula anywhere in this crate.
