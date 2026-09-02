# senken-plugin-mt5-hedging

A paper-trading adapter that simulates a **MetaTrader 5 hedging account**.

This is the account type behind almost every retail forex, gold and CFD
MT5 login: the mode MetaTrader 4 always used, and the one a retail trader
expects. A buy and a sell on the same symbol coexist as two separate
positions rather than netting into one.

The bar is that someone who has traded a real MT5 hedging account should
not be able to find a mechanical difference here, apart from money not
actually moving.

## What makes it that account rather than an average of three systems

- **Every deal opens its own ticket.** Positions are a list, not a map
  keyed by instrument. One symbol holds several at once, and one of them
  can be long while another is short — that locking is the whole reason a
  trader picks a hedging account.
- **Margin is per symbol.** `SYMBOL_TRADE_CALC_MODE` decides the formula:
  forex is `Lots × ContractSize / Leverage` and does not depend on the
  price at all, while a CFD's margin scales with the market price. One
  blanket `notional / leverage` would be right for forex and wrong for
  every CFD and every futures contract.
- **Margin call and stop out are different events.** A margin call blocks
  opening and closes nothing. A stop out closes the **biggest losing**
  position, looks again, and repeats until the margin level recovers — so
  a profitable hedge on the other side of a locked pair is not touched
  while a losing leg remains the largest loser.

## Broker numbers are settings, not constants

MT5 fixes the formulas; brokers fix the numbers. The margin call and stop
out thresholds, the contract size, the margin percentage and the swap
rates are all read per account from the symbol specification. None of them
has a default this simulator may invent, and none is written here from
memory of a broker's page.

## Status

Implemented and tested: the margin formulas, the four account figures
(balance, equity, margin used, free margin), the margin level percentage,
the ticket book, swap accrual with per-broker rollover and triple-swap
day, volume validation against the symbol's minimum, maximum, step and
total limit, MetaTrader's profit formula, and both risk events — a margin
call that blocks opening and closes nothing, and a stop-out loop that
closes the biggest loser, looks again, and stops as soon as the level
recovers.

Also implemented: a configurable commission model kept separate from swap
and market profit, partial close that leaves the ticket's number, open
price and accrued swap intact, and `close_by` for settling a locked pair
in one operation.

Not yet implemented: the order lifecycle (order types, deviation, stops
level, the order/deal/position history) and the `TradeAdapter`
implementation that joins all of this into a registrable adapter. Until
those land this crate is a set of correct calculations, not an account
anyone can trade — and this file will keep saying so.
