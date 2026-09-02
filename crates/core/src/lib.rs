//! Domain-agnostic primitives shared across Senken: one time type, the
//! fixed-point scaled-integer contract, and filesystem path encoding for
//! symbols that are otherwise untrusted input.
//!
//! This crate performs **no I/O**. Everything in it is a pure function or a
//! plain value type, so both `senken-marketdata` and the future bars/series
//! stack can depend on it without inheriting a runtime, a filesystem, or a
//! network stack. See the design record
//! for the
//! reasoning behind each type here.

pub mod decimal;
pub mod path_key;
pub mod range;
pub mod time;
pub mod zone;

pub use crate::decimal::{
    Scaled, checked_rescale, decimal_places, format_scaled, increment_from_precision,
    parse_increment, parse_scaled, plain_decimal,
};
pub use crate::path_key::{PathKeyError, path_key, symbol_from_path};
pub use crate::range::TimeRange;
pub use crate::time::{
    CivilDateTime, CivilDateTimeError, TimeError, UnixNanos, civil_from_days, days_from_civil,
    instant_from_civil,
};
pub use crate::zone::{IanaZone, UnknownZone};
