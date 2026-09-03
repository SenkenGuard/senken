# senken-plugin-spot

A paper-trading adapter that simulates a **spot exchange account** —
Binance, Bitget and OKX spot, closely enough to share one shape.

A spot account owns **asset balances, not directional exposure**. Buying
BTCUSDT does not open a long; it moves USDT into BTC. There is no
leverage, no liquidation, no borrowing, no short and no unrealised profit
in the futures sense.

## Two rules a leveraged simulator gets wrong when stretched over spot

- **You cannot sell what you do not hold.** A sell with insufficient base
  is refused, not shorted. There is nothing to be short of.
- **The fee comes out of the asset the trade produces** — base on a buy,
  quote on a sell — not out of one account currency. And when a venue's
  native-token discount is on, a *separate* asset absorbs it instead, so
  an adapter assuming "base on a buy" misreports every fill while that
  setting is enabled.

## Free and locked

A resting order locks exactly what it could consume if it filled
completely, which is what stops one balance being spent twice. Cancelling
releases the whole remaining lock; a partial fill releases only the slice
it consumed.

## What this crate proves about the kernel

This is the third settlement model and the one that tests the seam
hardest: a spot book has **no positions at all**, no risk state, nothing
to force-close, and nothing that time costs. It leaves `risk`, `enforce`
and `accrue` at their defaults, because writing empty implementations of
them would be the same as claiming it has them.

It compiles and passes with no line changed in `senken-sim-core`. Three
systems in, the seam is about settlement rather than about margin — which
is what it claimed to be.

## Status

Implemented and tested: the balance book with free and locked, order
locks and releases, buy and sell settlement, and the fee-currency rule
including the discount-asset case.

It is a registrable account: the `TradeAdapter` comes from
`senken-sim-core`'s shared one, so this crate supplies only what spot
itself decides — its settings, its capabilities, how to build its model,
and how to report its book. That whole adapter is one file with no
boilerplate in it.

Not yet implemented: the per-symbol filters (min notional, lot size, tick
size) that are the main source of real rejections, resting orders, and
`LIMIT_MAKER`/OCO. This file will keep saying so until they land.
