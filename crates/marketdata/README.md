# senken-marketdata

The market data layer of Senken, usable on its own:

- **Domain types** — `Instrument`, `InstrumentId` (`source:symbol`), and the
  fixed-point price/quantity contract they carry.
- **The source contract** — `MarketDataSource`, one trait a venue adapter
  implements to expose its instrument list.
- **`MarketData`** — a registry of sources that loads each catalog once,
  caches it on disk through `senken-storage`, and answers ranked,
  paginated, cross-venue searches.

```rust,ignore
use std::sync::Arc;
use senken_marketdata::{InstrumentQuery, MarketData};
use senken_storage::Storage;

let storage = Storage::new(".data");
storage.init()?;

let mut marketdata = MarketData::new(Arc::new(storage));
marketdata.register_source(Arc::new(my_source))?;

let page = marketdata.instruments(InstrumentQuery::new("btc").with_limit(20)).await;
for hit in &page.matches {
    println!("{} tick={}", hit.id, hit.instrument.tick_size);
}
```

Prices and quantities are integers at a fixed scale — see the documentation
on `Instrument` for the exact contract every source must honour.

## Cargo features

- `registry` *(default)* — `MarketData`, the source contract and the on-disk
  catalog cache; pulls in Tokio, chrono and `senken-storage`. Disable default
  features to get just the domain vocabulary (`Instrument`, `InstrumentId`,
  `InstrumentQuery`, the `decimal` helpers) with serde as the heaviest
  dependency.
