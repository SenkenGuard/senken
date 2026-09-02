# senken-plugin-futures

A paper-trading adapter that simulates a **crypto perpetual futures
account** — Binance USDⓈ-M and Bitget USDT-M, closely enough to share one
shape.

## What makes it this system

- **Liquidation**, triggered the way both venues state it: equity falling
  below maintenance margin. Not a margin-level percentage — a futures
  venue has no margin-call step between fine and closed.
- **Funding**, a periodic transfer between longs and shorts rather than a
  fee to the exchange. It settles straight against the balance: no order,
  no fill, no fee. A long pays a positive rate and a short is paid it, and
  the two net to nothing.
- **Two mode choices** that change what both of those mean: one-way or
  hedge, isolated or cross.

## The bracket table is data, and its absence is honest

Maintenance margin is not one rate. It steps up with position notional
from a per-symbol table the venue publishes and changes without notice.
Writing one from memory is exactly what this project forbids, so the table
is **supplied**. An account that has not been given one reports **no
liquidation price at all** — a trader shown a liquidation price believes
it, and a wrong one is worse than an absent one.

## The liquidation formula is labelled as derived

The venue publishes its expression as an image the research pass could not
read. What this crate carries is a first-principles derivation from the
equity-versus-maintenance-margin identity, cross-checked against the
page's own variable names. So `Liquidation` carries a `derived` flag, and
the label travels with the number rather than sitting in a comment nobody
reading a receipt will see. It must be confirmed against a live testnet
fill before it drives anything that trades.

## The funding interval is read, not assumed

Eight hours is the usual default, but venues shorten it during extreme
volatility and revert afterwards. A hard-coded eight hours undercounts by
eight to one against an hourly interval — which is why the interval is a
per-symbol setting and a test pins that difference.

## Status

Implemented and tested: the bracket table and its tiered maintenance
margin, the derived liquidation price with its label, the liquidation
trigger and its close-worst-first loop, funding with its direction and
interval, one-way versus hedge, and the `SettlementModel` implementation.

Not yet implemented: the `TradeAdapter` that would make this registrable,
cross-margin's dependence on every other open position, ADL, and
mark-versus-last trigger selection. This file will keep saying so until
they land.
