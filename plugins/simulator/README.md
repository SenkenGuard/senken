# senken-plugin-simulator

Senken's built-in paper-trading adapter.

It registers one `TradeAdapter` that trades **every instrument this
installation has a catalog for**, whichever venue the instrument came from.
That is the point of it: a strategy can be tested on a Kraken pair and a
Deribit option without holding an account at either, and a newly installed
venue plugin becomes paper-tradable with nothing here to update.

It is also the reference implementation of the trade contract — a settings
schema, custom actions, a capability declaration and order handling, in
about the space a real venue's adapter takes.

## What it is honest about

- **Cash-settled** against the account currency; it does not custody base
  assets. One settlement model covers spot, perpetuals and FX at once, at
  the cost of a spot account behaving like a margin account at 1×.
- **Resting orders match against the mark, when the account is read** — not
  against an order book, because there is no depth to match into. A limit
  fills at its own price; a stop becomes a market order and takes the mark.
- **A market order fills at the mark plus a fixed slippage in basis
  points**, always against the trader. That is the whole model.
- **It reaches no network.** Prices come from whatever `MarkPriceSource` the
  engine was assembled with.

## Settings

Account currency, starting balance, leverage, fee (bps) and slippage (bps),
declared as a `SettingsSchema` the web client renders a form from.

An **Access** field (`Trading` / `Read-only (investor)`) is part of that same
form — attaching or re-editing an account can flip it between the two at
will, which is what makes the simulator useful for testing the read-only
path itself, not only for demonstrating it. `account_access` reports
`AccessLevel::ReadOnly` with a note ("This account was attached
read-only.") whenever the field is set, and `AccessLevel::Trade` otherwise;
this is the same per-account narrowing an MT5 investor login forces on a
real broker, reproduced here without needing one.

## Actions

`deposit` (with an amount form) and `reset`, which returns the account to
its starting balance and clears every position and order.

## Amendment support

The simulator declares `AdapterFeature::ModifyOrders` and implements
`modify_order`: a resting order's quantity, limit price or trigger price can
be changed in place, rejected with `TradeError::OrderNotOpen` once the order
has already filled or been cancelled. An amendment is itself allowed to
trigger a fill — changing a stop's trigger to a level the current mark
already satisfies settles it immediately rather than waiting for the next
unrelated read to notice.

## Health

`health` reports `Connected` once an account's books exist, and `Degraded`
(never `Disconnected` — nothing here can lose network) when the attachment
exists but its book snapshot does not, which happens if the on-disk snapshot
was lost or a fresh install is restoring an old accounts database. The
message names the fix: save the account's settings once to create its
books.

## Storage

One atomic JSON snapshot per installation at `trade/simulator/books.json`,
written through `senken-storage` like every other piece of on-disk state.
