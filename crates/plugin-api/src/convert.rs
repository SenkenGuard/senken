//! Conversions between this crate's WIT wire types and the primitive
//! fields Senken's own domain types are built from.
//!
//! This module deliberately does not import `senken_core` or
//! `senken_series` — this crate's own `Cargo.toml` must never gain a
//! `senken-*` dependency (see `tests/no_domain_dependency.rs`), because
//! publishing this SDK with one would publish that crate's implementation
//! alongside it. Instead, every function here takes and returns the exact
//! primitives those domain types are already built from: a raw `i64`
//! nanosecond count for `senken_core::UnixNanos`, a `(scale, value)` pair
//! for `senken_core::Scaled`, and a field-mirroring [`BarFields`] for
//! `senken_series::Bar`. The one crate that depends on both sides
//! (`crates/plugin-host`) wires them together with a direct field-by-field
//! call — `instant_from_nanos(bar.ts_open.as_nanos())`, and so on — so the
//! conversion logic itself still lives in exactly one place, written once
//! and tested here, without either crate's internal shape becoming the
//! other's public contract.

use crate::{Bar, BarSpec, BarUnit, Scaled, Volume};

/// Converts a `senken_core::UnixNanos`'s raw nanosecond count into the WIT
/// `instant` it crosses the boundary as.
///
/// `instant` is a plain `s64` alias for nanoseconds since the epoch — the
/// same unit, same zero point, same zero-tolerance-for-other-units rule
/// `UnixNanos` itself enforces — so this is a named boundary crossing
/// rather than a computation, for the same reason `UnixNanos` has no
/// `From<i64>`: a call site should always say which unit it means.
#[must_use]
pub const fn instant_from_nanos(nanos: i64) -> i64 {
    nanos
}

/// The inverse of [`instant_from_nanos`].
#[must_use]
pub const fn nanos_from_instant(instant: i64) -> i64 {
    instant
}

/// Converts a `(scale, value)` pair — `senken_core::Scaled`'s own two
/// fields, or any instrument's `price_scale`/`qty_scale` paired with a raw
/// integer — into the WIT `scaled` record.
#[must_use]
pub const fn scaled_from_parts(scale: u8, value: i64) -> Scaled {
    Scaled { scale, value }
}

/// The inverse of [`scaled_from_parts`], returned as `(scale, value)`.
#[must_use]
pub const fn parts_from_scaled(scaled: Scaled) -> (u8, i64) {
    (scaled.scale, scaled.value)
}

/// Mirrors `senken_series::BarUnit`'s six cases exactly, without depending
/// on that crate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BarUnitFields {
    /// One second.
    Second,
    /// One minute.
    Minute,
    /// One hour.
    Hour,
    /// One calendar day.
    Day,
    /// Seven days.
    Week,
    /// One calendar month.
    Month,
}

impl From<BarUnitFields> for BarUnit {
    fn from(unit: BarUnitFields) -> Self {
        match unit {
            BarUnitFields::Second => Self::Second,
            BarUnitFields::Minute => Self::Minute,
            BarUnitFields::Hour => Self::Hour,
            BarUnitFields::Day => Self::Day,
            BarUnitFields::Week => Self::Week,
            BarUnitFields::Month => Self::Month,
        }
    }
}

impl From<BarUnit> for BarUnitFields {
    fn from(unit: BarUnit) -> Self {
        match unit {
            BarUnit::Second => Self::Second,
            BarUnit::Minute => Self::Minute,
            BarUnit::Hour => Self::Hour,
            BarUnit::Day => Self::Day,
            BarUnit::Week => Self::Week,
            BarUnit::Month => Self::Month,
        }
    }
}

/// Mirrors `senken_series::Volume`'s three cases exactly, without
/// depending on that crate. `Real`'s value is the raw traded quantity at
/// whatever scale the caller already tracks; `Tick` is a count of price
/// changes, never an asset quantity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeFields {
    /// Base-asset quantity actually traded, at `qty_scale`.
    Real(i64),
    /// Number of price changes in the interval.
    Tick(u32),
    /// The source did not report volume.
    Absent,
}

/// Mirrors `senken_series::Bar` field-for-field, plus the `price_scale`
/// and `qty_scale` a real `Bar` does not carry on itself (that pair lives
/// on the series a bar belongs to — an `Instrument` or a `SeriesKey`), and
/// the `BarSpec` a real `Bar` likewise leans on external context for. A
/// guest has no channel to that external context, so both travel with
/// every bar at this boundary; see `bar`'s own doc comment in
/// `wit/senken.wit` for why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BarFields {
    /// `senken_core::UnixNanos::as_nanos()` of the bar's `ts_open`.
    pub ts_open_nanos: i64,
    /// The series' `BarSpec::step`.
    pub spec_step: u32,
    /// The series' `BarSpec::unit`.
    pub spec_unit: BarUnitFields,
    /// The series' price scale.
    pub price_scale: u8,
    /// `Bar::open`, at `price_scale`.
    pub open: i64,
    /// `Bar::high`, at `price_scale`.
    pub high: i64,
    /// `Bar::low`, at `price_scale`.
    pub low: i64,
    /// `Bar::close`, at `price_scale`.
    pub close: i64,
    /// The series' quantity scale.
    pub qty_scale: u8,
    /// `Bar::volume`.
    pub volume: VolumeFields,
    /// `Bar::quote_volume`, at `qty_scale`.
    pub quote_volume: Option<i64>,
    /// `Bar::trade_count`.
    pub trade_count: Option<u32>,
    /// `Bar::taker_buy_volume`, at `qty_scale`.
    pub taker_buy_volume: Option<i64>,
}

impl From<BarFields> for Bar {
    fn from(fields: BarFields) -> Self {
        Self {
            ts_open: instant_from_nanos(fields.ts_open_nanos),
            spec: BarSpec {
                step: fields.spec_step,
                unit: fields.spec_unit.into(),
            },
            open: scaled_from_parts(fields.price_scale, fields.open),
            high: scaled_from_parts(fields.price_scale, fields.high),
            low: scaled_from_parts(fields.price_scale, fields.low),
            close: scaled_from_parts(fields.price_scale, fields.close),
            volume: match fields.volume {
                VolumeFields::Real(value) => {
                    Volume::Real(scaled_from_parts(fields.qty_scale, value))
                }
                VolumeFields::Tick(count) => Volume::Tick(count),
                VolumeFields::Absent => Volume::Absent,
            },
            quote_volume: fields
                .quote_volume
                .map(|value| scaled_from_parts(fields.qty_scale, value)),
            trade_count: fields.trade_count,
            taker_buy_volume: fields
                .taker_buy_volume
                .map(|value| scaled_from_parts(fields.qty_scale, value)),
        }
    }
}

/// Why a WIT [`Bar`] could not be converted back to [`BarFields`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum BarFieldsError {
    /// `open`, `high`, `low` and `close` did not all share one scale.
    ///
    /// A real `senken_series::Bar` has exactly one price scale for all
    /// four fields (it lives once on the series, not once per field), so
    /// a WIT `bar` whose four `scaled` prices disagree cannot have
    /// originated from one — this is rejected rather than silently
    /// rescaled, the same discipline `checked_rescale` uses internally.
    #[error("open/high/low/close do not share one scale: {0:?}")]
    MixedPriceScale((u8, u8, u8, u8)),
    /// A quantity field (`volume`, `quote_volume` or `taker_buy_volume`)
    /// did not use the same scale as the others.
    #[error("a quantity field does not share the bar's quantity scale")]
    MixedQuantityScale,
}

impl TryFrom<Bar> for BarFields {
    type Error = BarFieldsError;

    fn try_from(bar: Bar) -> Result<Self, Self::Error> {
        let (price_scale, open) = parts_from_scaled(bar.open);
        let (high_scale, high) = parts_from_scaled(bar.high);
        let (low_scale, low) = parts_from_scaled(bar.low);
        let (close_scale, close) = parts_from_scaled(bar.close);
        if price_scale != high_scale || price_scale != low_scale || price_scale != close_scale {
            return Err(BarFieldsError::MixedPriceScale((
                price_scale,
                high_scale,
                low_scale,
                close_scale,
            )));
        }

        let mut qty_scale = None;
        let mut take_qty_scale = |scale: u8| -> Result<(), BarFieldsError> {
            match qty_scale {
                Some(existing) if existing != scale => Err(BarFieldsError::MixedQuantityScale),
                Some(_) => Ok(()),
                None => {
                    qty_scale = Some(scale);
                    Ok(())
                }
            }
        };

        let volume = match bar.volume {
            Volume::Real(scaled) => {
                let (scale, value) = parts_from_scaled(scaled);
                take_qty_scale(scale)?;
                VolumeFields::Real(value)
            }
            Volume::Tick(count) => VolumeFields::Tick(count),
            Volume::Absent => VolumeFields::Absent,
        };
        let quote_volume = bar
            .quote_volume
            .map(|scaled| {
                let (scale, value) = parts_from_scaled(scaled);
                take_qty_scale(scale).map(|()| value)
            })
            .transpose()?;
        let taker_buy_volume = bar
            .taker_buy_volume
            .map(|scaled| {
                let (scale, value) = parts_from_scaled(scaled);
                take_qty_scale(scale).map(|()| value)
            })
            .transpose()?;

        Ok(Self {
            ts_open_nanos: nanos_from_instant(bar.ts_open),
            spec_step: bar.spec.step,
            spec_unit: bar.spec.unit.into(),
            price_scale,
            open,
            high,
            low,
            close,
            // A bar with no volume field at all (`Volume::Absent` and no
            // quote/taker-buy volume) never observed a quantity scale;
            // `0` is an inert placeholder that round-trips correctly since
            // nothing at this scale is ever read back out.
            qty_scale: qty_scale.unwrap_or(0),
            volume,
            quote_volume,
            trade_count: bar.trade_count,
            taker_buy_volume,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{BarFields, BarUnitFields, VolumeFields, instant_from_nanos, scaled_from_parts};
    use crate::Bar;

    #[test]
    fn instant_and_nanos_are_the_same_number() {
        assert_eq!(
            super::nanos_from_instant(super::instant_from_nanos(123)),
            123
        );
    }

    #[test]
    fn scaled_round_trips_through_parts() {
        let scaled = scaled_from_parts(8, -42);
        assert_eq!(super::parts_from_scaled(scaled), (8, -42));
    }

    /// The scale in this fixture — 12 fractional digits — is not a made-up
    /// number: it is BitMart's own reported `price_max_precision` for a
    /// spot symbol (`plugins/bitmart/tests/fixtures/spot.json`), the
    /// deepest scale any onboarded venue in this workspace actually
    /// reports. A round trip that only proves itself at scale 2 or 8 would
    /// not have caught a bug that only appears once a scale needs more
    /// than one byte's worth of digits to express.
    fn bitmart_spot_scale_fixture() -> BarFields {
        BarFields {
            ts_open_nanos: 1_788_048_000_000_000_000,
            spec_step: 15,
            spec_unit: BarUnitFields::Minute,
            price_scale: 12,
            open: 123_456_789_012,
            high: 123_999_999_999,
            low: 123_000_000_001,
            close: 123_500_000_000,
            qty_scale: 8,
            volume: VolumeFields::Real(700_000_000),
            quote_volume: Some(1_234_567_890_123),
            trade_count: Some(42),
            taker_buy_volume: Some(300_000_000),
        }
    }

    #[test]
    fn bar_round_trips_through_wit_at_an_uncommon_venue_scale() {
        let fields = bitmart_spot_scale_fixture();
        let wit_bar: Bar = fields.into();
        let back = BarFields::try_from(wit_bar).expect("a bar this crate built must convert back");
        assert_eq!(back, fields);
    }

    #[test]
    fn a_bar_with_only_absent_volume_round_trips_too() {
        let fields = BarFields {
            volume: VolumeFields::Absent,
            quote_volume: None,
            taker_buy_volume: None,
            trade_count: None,
            // No quantity field is present, so no quantity scale is ever
            // observed on the way through — the placeholder documented on
            // `qty_scale`'s conversion, not a scale this bar actually had.
            qty_scale: 0,
            ..bitmart_spot_scale_fixture()
        };
        let wit_bar: Bar = fields.into();
        let back = BarFields::try_from(wit_bar).expect("an absent-volume bar must convert back");
        assert_eq!(back, fields);
    }

    #[test]
    fn a_bar_with_tick_volume_round_trips_without_a_quantity_scale() {
        let fields = BarFields {
            volume: VolumeFields::Tick(9),
            quote_volume: None,
            taker_buy_volume: None,
            qty_scale: 0,
            ..bitmart_spot_scale_fixture()
        };
        let wit_bar: Bar = fields.into();
        let back = BarFields::try_from(wit_bar).expect("a tick-volume bar must convert back");
        assert_eq!(back, fields);
    }

    /// Proves `MixedPriceScale` is actually reachable, not just declared:
    /// a `bar` whose four prices disagree on scale cannot have come from
    /// one real `senken_series::Bar` (which has exactly one price scale
    /// for all four), so converting it back must be refused rather than
    /// silently picking one of the four scales.
    #[test]
    fn mismatched_price_scales_are_rejected_not_silently_resolved() {
        let mut wit_bar: Bar = bitmart_spot_scale_fixture().into();
        wit_bar.high = scaled_from_parts(2, 12_399);
        let err = BarFields::try_from(wit_bar).unwrap_err();
        assert!(matches!(err, super::BarFieldsError::MixedPriceScale(_)));
    }

    /// Same property as the price-scale guard above, proven for the
    /// quantity side: `volume` and `quote_volume` disagreeing on scale
    /// cannot have come from one real `Bar` either.
    #[test]
    fn mismatched_quantity_scales_are_rejected_not_silently_resolved() {
        let mut wit_bar: Bar = bitmart_spot_scale_fixture().into();
        wit_bar.quote_volume = Some(scaled_from_parts(2, 42));
        let err = BarFields::try_from(wit_bar).unwrap_err();
        assert!(matches!(err, super::BarFieldsError::MixedQuantityScale));
    }

    #[test]
    fn instant_from_nanos_never_rescales() {
        // Unlike `scaled`, `instant` carries no scale of its own to get
        // wrong — this is the property that makes it safe for
        // `instant_from_nanos`/`nanos_from_instant` to be the identity
        // function rather than a fallible conversion.
        for nanos in [0, 1, -1, i64::MIN, i64::MAX] {
            assert_eq!(super::nanos_from_instant(instant_from_nanos(nanos)), nanos);
        }
    }
}
