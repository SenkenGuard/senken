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

## Cargo features

- *(none)* — the contract and its vocabulary. An adapter crate needs only
  this.
- `engine` — `TradeEngine`, the registry and its validation.
- `accounts` — `TradeAccountStore`, the attached accounts as guarded SQLite
  queries against `senken-identity`'s own database.
