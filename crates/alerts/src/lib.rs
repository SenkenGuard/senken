//! Standalone alerts: `(series key, indicator
//! spec, condition)` rows that lease the series themselves and survive the
//! chart that created them being closed.
//!
//! # An alert is not a hidden chart
//!
//! The one architectural rule this whole crate exists to uphold: an alert
//! must never be implemented as a background chart session, because that
//! would make it inherit a chart's lifetime and layout concerns — closing a
//! chart, or a laptop, would kill the alert. Nothing here has a
//! back-reference to a chart, a pane or a workspace of any kind. What
//! charts and alerts genuinely share is the **indicator computation**
//! (`senken-indicators`,), not a session — both are just two
//! different consumers of the same incremental evaluation over the same
//! series.
//!
//! [`AlertRunner`] is the proof of this in code: it holds its own
//! [`senken_subscription::Lease`] on the instrument it watches, obtained
//! from a [`senken_subscription::SubscriptionPool`] exactly the way a chart
//! pane, a watchlist row or a position would ("chart panes, watchlist rows, alerts and open positions are all just leaseholders").
//! Dropping some other lease on the same instrument — a chart's, say — has
//! no effect on an `AlertRunner`'s own lease; the pool only unsubscribes
//! from the venue once every lease on an instrument, from whatever source,
//! has been dropped.
//!
//! [`AlertEngine`] is what actually keeps a runner alive for the server's
//! whole lifetime: it reconciles [`AlertStore`]'s enabled rows against a
//! [`senken_subscription::SubscriptionPool`] per instrument source at
//! startup, and its own `register`/`unregister` keep that in sync with
//! every later create/delete — the piece that makes "an alert outlives the
//! chart that created it" true of the running server, not only of a unit
//! test holding both leases by hand.
//!
//! # Never a warm-up artefact, never a forming bar
//!
//! Two rules the scope note calls out by name, both enforced by
//! construction rather than by caller discipline:
//!
//! - **[`AlertEvaluator`] never reads a condition before its indicator
//!   reports [`initialized`](senken_indicators::Indicator::initialized).**
//!   An EMA's first output is not an EMA; this crate never
//!   compares one against a threshold.
//! - **[`TickBarBuilder`] never hands a forming bucket to the evaluator.**
//!   It follows the exact same "never emit a partial bucket" discipline
//!   `senken_series::Aggregator` already uses for folding finer bars into
//!   coarser ones (a bar is only knowable at
//!   `ts_open + interval`) — see that type's own module docs for the
//!   mechanism.
//!
//! # Persistence
//!
//! [`AlertStore`] follows exactly the guarded-query pattern
//! `senken-chart` established for alerts are owned SQLite
//! records sharing `senken-identity`'s accounts database (via
//! [`senken_identity::IdentityStore::shared_connection`]) rather than a
//! second database or a JSON blob, and every caller-facing method takes a
//! [`senken_identity::AuthenticatedUser`] and turns its resolved
//! [`senken_acl::Scope`] into a `WHERE` clause. See
//! [`AlertStore::all_enabled_for_engine`]/[`AlertStore::record_fire`]'s own
//! docs for the one deliberate exception: the evaluation engine's internal
//! bookkeeping, which answers "what does the server need to keep running"
//! rather than "what can this caller see", and so is not gated the same
//! way.
//!
//! # Out of scope
//!
//! Notification delivery of any kind — firing is recording that it fired,
//! nothing more. Conditions on anything but bars and indicators.

mod bar_builder;
mod condition;
mod engine;
mod error;
mod evaluator;
mod id;
mod indicator_spec;
mod runner;
mod store;

pub use crate::bar_builder::TickBarBuilder;
pub use crate::condition::{Comparator, Condition, IndicatorField};
pub use crate::engine::AlertEngine;
pub use crate::error::{AlertError, IndicatorSpecError};
pub use crate::evaluator::{AlertEvaluator, Fired};
pub use crate::id::AlertId;
pub use crate::indicator_spec::{ConcreteIndicator, IndicatorSpec};
pub use crate::runner::AlertRunner;
pub use crate::store::{AlertRecord, AlertStore};

// Re-exported for convenience, exactly like `senken-chart` re-exports
// it: every listing here returns `senken_identity::Page<T>`, so a caller of
// both crates' listings is not asked to learn two names for one concept.
pub use senken_identity::Page;
