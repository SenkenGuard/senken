# senken-trade

The Senken trade engine: the contract a broker, exchange or simulator
implements, the vocabulary it speaks, and the registry of adapters and
user-attached accounts in front of it.

```text
  plugin  ──registers──▶  TradeAdapter  ──registered in──▶  TradeEngine
                                ▲                                │
                                │                         validates, routes
                                │                                ▼
                      TradeContext (time, catalog,      TradeAccountStore
                       mark price) per call             (whose account,
                                                         which settings)
```

- **The adapter owns the money; the engine owns the attachment.** A real
  broker already holds the authoritative orders, positions and balances for
  an account, so those are read through the adapter on every request and
  none of them are stored here. What Senken stores is which adapter, whose
  account, under what label, with which settings.
- **The vocabulary is broad; each adapter declares its part of it.**
  `AdapterCapabilities` and `InstrumentCoverage` let a crypto spot exchange,
  a perpetual-futures venue, an FX broker quoting lots and a paper simulator
  all speak the same contract. The engine refuses what an adapter cannot
  serve before anything is sent, and the order ticket renders only the
  controls that mean something for the account in front of the user.
- **A plugin describes its settings; it never ships user interface.** A
  `SettingsSchema` and an `AdapterAction` are data: the server validates
  against them, the client renders a form from them, and no plugin author is
  ever handed the session of a user who opens its settings screen.
- **Credentials cannot leave by accident.** `SecretString` serialises as
  `null` — always, with no flag to change it — so an API response or a log
  line cannot carry one. Persisting one goes through
  `SettingsValues::to_storage_json`, which the account store is the only
  caller of.
- **Trading is owner-only, whatever the role says.** `Scope::All` widens
  what an operator can see about an account; it never widens reading the
  credential inside it or sending an order with it.
- **Money is exact.** Every price, quantity, balance and fee is a
  `(scale, value)` integer pair. There is no `f64` in the crate.

## Access is per account, not per adapter

`AdapterCapabilities` is the most an adapter can ever do — what an adapter
card shows before any account exists. `AccountAccess` narrows that to one
account: a MetaTrader 5 investor login, an exchange key minted without trade
scope, and a demo account past its trial all report the same adapter but a
different `AccessLevel` (`Trade` or `ReadOnly`). Every mutating call —
`place_order`, `cancel_order`, `modify_order`, `close_position`,
`run_action` — resolves the account's own access first and refuses anything
short of `AccessLevel::Trade` before the adapter is asked at all, so a
read-only login learns it cannot trade from the engine, not from a rejection
at the venue. `AccessLevel` is `#[non_exhaustive]`: a variant a build does
not know about yet reads as not-trading, never as trading by default.

## What the engine validates before an adapter is called

`place_order` and `close_position` round every price to the instrument's
tick and the quantity to its step — as scaled integers, never through a
float — before the request reaches an adapter; an indicator-derived price of
`68420.1379` becomes `68420.13` at this boundary. `modify_order` rounds the
same way. Both also check the account's *resolved* capabilities, not the
adapter's own maximum, so an order kind the adapter supports in general but
this particular account may not is refused here rather than reaching the
adapter and being taken at its word.

## Closing and amending a position or order

`TradeEngine::close_position` sends an opposite market order sized to
exactly what the adapter's own `positions` call reports held **at the
moment of the call**, never a size a screen was holding — a position that
moved between the table being drawn and the button being pressed closes at
what it is now. `reduce_only` is set on the closing order only when the
account's resolved capabilities include `AdapterFeature::ReduceOnly`; a
spot-holdings adapter has no such flag and is never sent one.

`TradeEngine::modify_order` is what backs `AdapterFeature::ModifyOrders`:
refused first for a read-only account, then for an account whose resolved
capabilities do not declare the feature, then for an amendment that changes
nothing, then for an amendment naming a price the order's own kind does not
carry (a limit price on a market order, a trigger on a plain limit). Only
past all four is the adapter's own `modify_order` called.

## An adapter's refresh cost

A screen watching one account calls balances, positions, orders and fills
separately — four requests per refresh, and behind a real broker each is a
real venue call. At Senken's five-second poll tick (`lib/trade/poller.ts` on
the web side) that is 48 requests a minute for every account being watched;
`lib/trade/watch-scope.ts` keeps that to the accounts a screen actually
shows rather than every attached account, but the four-calls-per-account
cost itself is not reduced by that. The simulator is local and does not
notice; a real venue's own rate limit will.

There is no adapter-declared refresh cadence yet — every adapter is polled
at the same fixed interval regardless of what it can actually sustain. The
right fix is the same shape `senken_venue`'s `LimitGroup` already gives
market data: an adapter declaring its own budget, the engine or the poller
honouring it. That is future work, not yet built here.

## Cargo features

- *(none)* — the contract and its vocabulary. An adapter crate needs only
  this.
- `engine` — `TradeEngine`, the registry and its validation.
- `accounts` — `TradeAccountStore`, the attached accounts as guarded SQLite
  queries against `senken-identity`'s own database.
