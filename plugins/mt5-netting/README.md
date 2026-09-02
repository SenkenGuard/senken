# senken-plugin-mt5-netting

A paper-trading adapter that simulates a **MetaTrader 5 netting account**.

Netting is a position-accounting rule: the account holds **at most one
position per symbol**. Every trade in that symbol folds into it rather
than sitting beside it. It is MetaTrader's netting system, an exchange
account's accounting, and what a crypto perpetual venue's one-way mode
resembles.

## The four transitions, and there is no fifth

Every fill after the first is exactly one of: **add** (volume-weighted
average, ticket unchanged), **partial reduce** (profit booked, entry left
alone), **flat** (closed exactly), or **reversal**. A hedging account's
close-by does not exist here, because there is never a second ticket to
close against.

## Why a position carries two identifiers

A reversal changes `POSITION_TICKET` to the reversing order's own, because
the exposure that now exists was opened by that order — but
`POSITION_IDENTIFIER` **survives**, because everything that groups a
position's deal history keys on it. Collapsing them into one field makes a
reversal either break the history or lie about when the current exposure
opened.

Two more things a reversal gets wrong easily, both checked by breaking
them: the new position is priced at the **reversing deal's own price**,
not a weighted average with the leg that just closed; and the open time
**resets**, because the surviving exposure did not exist before.

## What this crate is really for

`senken-sim-core`'s seam claims a second trading system can be added as
one settlement model with **no edit to the kernel**. This is that second
system, and it was written to test the claim: it compiles and passes
without a line changed in `senken-sim-core`. Had it needed to reach in,
the seam would have been in the wrong place — and cheaper to move at the
second system than at the fourth.

## Status

Implemented and tested: the four transitions, the two identifiers, the
realised ledger that survives a position closing, and the `SettlementModel`
implementation.

Not yet implemented: the `TradeAdapter` that would make this a registrable
account, its settings schema, and margin. This file will keep saying so
until they land.
