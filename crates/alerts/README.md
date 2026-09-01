# senken-alerts

Standalone price/indicator alerts for Senken:
`(series key, indicator spec, condition)` rows that lease the series
themselves and survive the chart that created them being closed.

- **Not a hidden chart.** An alert holds its own
  `senken_subscription::Lease` on the instrument it watches, obtained from
  the same `SubscriptionPool` a chart pane, watchlist row or position uses.
  There is no back-reference to a chart anywhere in this crate — dropping a
  chart's lease has no effect on an alert's own.
- **Never a warm-up artefact.** `AlertEvaluator` never reads its condition
  before the wrapped `senken-indicators::Indicator` reports `initialized()`.
- **Never a forming bar.** `TickBarBuilder` folds live price ticks into
  closed bars using the same "never emit a partial bucket" discipline
  `senken_series::Aggregator` already uses for finer-to-coarser bar
  aggregation.
- **The same guarded-query pattern as `senken-chart`/`senken-identity`.**
  Every caller-facing method on `AlertStore` takes an `AuthenticatedUser`
  and turns its resolved `Scope` into a `WHERE` clause, including in every
  listing's total row count. The one deliberate exception is the small,
  explicitly un-guarded surface the evaluation engine itself needs
  (`all_enabled_for_engine`/`record_fire`) — see their own doc comments for
  why answering "what does the server need to keep running" is a different
  question from "what can this caller see".
- **Shares `senken-identity`'s accounts database**, the same way
  `senken-chart` does — alerts reference `users(id)`, so their table
  lives in the same SQLite file rather than a second one.
- **Out of scope**: notification delivery of any kind (firing is recording
  that it fired) and conditions on anything but bars and indicators.
