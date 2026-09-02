# senken-sim-core

The shared simulation kernel Senken's paper-trading adapters are built on.

Four simulated trading systems — an MT5 hedging account, an MT5 netting
account, a crypto perpetual futures account and a spot account — disagree
about exactly one thing: **what a fill does to the account's state.**
Everything else a simulator does is the same in all four.

So this crate holds everything else, and the difference is one trait:

```
                 senken-trade          the contract
                       ▲
                       │
                 senken-sim-core       order intake, resting orders,
                       ▲               fill pricing, fees, history
                       │
                 SettlementModel       what a fill does to the book
          ┌────────────┼────────────┬────────────┐
      mt5-hedging  mt5-netting   futures       spot
```

## Why a crate and not a module

A plugin author outside this repository should be able to simulate a fifth
system — a different broker's rules, an exchange Senken does not ship —
without vendoring this code. That is the same reason the rest of
`crates/` are crates.

## Why the seam is settlement

Reading the netting simulator this kernel was extracted from, its `execute`
was already almost entirely generic: it delegated one call and then did the
same six things every system does — move cash by the realised amount,
subtract the fee, mark the order filled, record its average price, push a
fill, stamp the time. That one call *is* the netting rule, and it is the
whole difference between a netting book and a hedging one.

The measure of whether this seam is in the right place is what a fifth
system costs: one file implementing `SettlementModel`, its settings schema,
its capability declaration and its tests — with no edit to this crate. If a
fifth system cannot be added without changing the kernel, the seam is
wrong.

## Money

Nothing here touches a float. Every function works in `i128` and lands back
on an `i64` at a declared scale. A paper account whose arithmetic drifts
teaches its user the wrong thing about their strategy — and the arithmetic
lives here rather than in each adapter because a fee rounding fixed in one
simulator and not another is a bug nobody can see.
