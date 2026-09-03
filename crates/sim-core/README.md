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

## What the seam covers

`SettlementModel` carries four things, because those are the four a
trading system genuinely decides for itself:

- **`settle`** — what a fill does to the book. A hedging book opens a
  ticket, a netting book merges and reverses, a spot book moves two asset
  balances.
- **`risk`** — what the account's danger is measured in. MetaTrader has
  one margin level for the whole account; a futures venue has a
  liquidation price per isolated position; spot has none, because nothing
  is borrowed.
- **`enforce`** — what the system does about a breach. A stop out closes
  the biggest loser and repeats; a liquidation takes the position whose
  maintenance margin is gone; spot closes nothing, ever.
- **`accrue`** — what time itself costs. Swap, funding, or nothing. It
  takes a *range*, so a book left unread for a week accrues the week
  rather than one night.

The last three default to "nothing happens", so a system with no risk to
measure does not have to write an empty answer for one — which is what
keeps spot from carrying margin vocabulary it has no use for.

## Settling through time, not at a point

A resting order evaluated against the single price that happened to be
current when someone read the account is not a simulation of resting. A
stop fills at the reader's price rather than at the bar that touched it,
and if nobody looks for an hour, the stop fills an hour late — the
trader's real risk was never modelled.

Senken stores bars, which almost nothing in this class of application
does. So settlement replays the bars between the book's own
`settled_through` and now, in order: a level is reached by the bar whose
high or low actually reached it, at that bar's own time, and the range is
half-open at the start so reading twice settles nothing again.

**Intrabar order is unknowable and this says so.** Within one bar, whether
the high or the low came first cannot be recovered. When a bar reaches
both a stop loss and a take profit, the **worse-for-the-trader** side is
taken first — because assuming the profitable one would flatter every
strategy replayed through it. Finer bars narrow that window; they never
close it.

## Pending orders

A limit, stop or stop-limit **rests**. Filling one at the mark would make
the capability a lie — a trader who places a limit below the market and
watches it fill immediately has learned nothing true about their strategy.
A resting order fills at **its own price**, never at the mark: the market
reaching a level is not the same as the market crossing it.

A venue can hold something against a resting order — `reserve` and
`release` — which is how a spot account locks the balance an order could
consume while a margined one, holding margin against positions rather
than orders, holds nothing and does not have to say so.

Cancelling and amending live here too, not in each venue, so a venue that
forgot to declare them would not advertise less than the adapter can do.
An amendment keeps the order's identity: it is the same order at a new
price, not a cancel and a replace.

A level is reached by the **bar whose high or low actually reached it**,
at that bar's own time. A stop hit an hour ago fills an hour ago, at its
own stop, even though the reader arriving now sees only the current price
and no sign anything happened. Where an installation holds no bars for the
instrument the current mark is the honest fallback, and an order then
fills when a read first sees it reachable.

A buy stop is reached from **below** and a sell stop from **above**, so
the two read different extremes of the same bar. Evaluating either on the
close alone misses every level a bar traded through and came back from.

## Money

Nothing here touches a float. Every function works in `i128` and lands back
on an `i64` at a declared scale. A paper account whose arithmetic drifts
teaches its user the wrong thing about their strategy — and the arithmetic
lives here rather than in each adapter because a fee rounding fixed in one
simulator and not another is a bug nobody can see.
