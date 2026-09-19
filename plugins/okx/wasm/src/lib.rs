//! OKX's `venue-plugin` component: spot instruments and bars, fetched
//! entirely through the host's `fetch` — this crate never imports
//! `wasi:sockets` or `wasi:http`, and has no way to (`wit/senken.wit`'s
//! `venue-plugin` world only ever imports `http`).
//!
//! Every byte this component decodes goes through `okx-core`'s parsing —
//! the same parsing `plugins/okx`'s native adapter calls for its own spot
//! source — so this file is a thin wrapper: build the request path, call
//! [`fetch`], hand the bytes to `okx-core`, translate its neutral result
//! into the WIT wire types. See `okx-core`'s own module docs for the venue
//! facts (row shape, the 100-row cap, the `after`/`before` pagination
//! inversion) neither side hand-writes twice.
//!
//! Spot only, not swap/futures/options: `wit/senken.wit`'s `instrument`
//! record has no field for a derivative's contract terms, so this
//! component's catalog is exactly what `okx_core::parse_spot_instruments`
//! returns and nothing else. `plugins/okx`'s native adapter still serves
//! the other markets, plus every market's order book and the live feed —
//! see that crate's own `Plugin::activate_with_http` for the split.

use senken_plugin_api::{
    Bar, BarSpec, BarUnit, Instrument, InstrumentKind, InstrumentStatus, Scaled, VenueDescriptor,
    VenueError, VenueGuest, Volume, export_venue, fetch,
};

struct OkxVenue;

impl VenueGuest for OkxVenue {
    fn descriptor() -> VenueDescriptor {
        VenueDescriptor {
            // The same source id the compiled-in adapter served spot under
            // before this component took that job. A saved chart layout
            // stores the instrument id it was drawn for, and the source is
            // half of that id — renaming the source here would orphan every
            // layout anyone had already saved, silently, with the chart
            // simply reporting the instrument as gone.
            id: "okx-spot".to_owned(),
            name: "OKX".to_owned(),
            base_url: "https://www.okx.com".to_owned(),
        }
    }

    fn instruments() -> Result<Vec<Instrument>, VenueError> {
        let body =
            fetch("/api/v5/public/instruments?instType=SPOT", 1).map_err(VenueError::Fetch)?;
        let instruments = okx_core::parse_spot_instruments(&body).map_err(to_venue_error)?;
        Ok(instruments.into_iter().map(to_wit_instrument).collect())
    }

    fn supported_specs() -> Vec<BarSpec> {
        okx_core::supported_bar_specs()
            .into_iter()
            .map(to_wit_bar_spec)
            .collect()
    }

    fn max_rows() -> u32 {
        okx_core::HISTORY_CANDLES_MAX_ROWS
    }

    fn bars(
        source_symbol: String,
        spec: BarSpec,
        range_start: i64,
        range_end: i64,
    ) -> Result<Vec<Bar>, VenueError> {
        if range_start >= range_end {
            return Ok(Vec::new());
        }
        let core_spec = from_wit_bar_spec(spec);
        let interval = okx_core::okx_interval(core_spec)
            .ok_or_else(|| VenueError::Rejected(format!("unsupported bar spec {core_spec:?}")))?;
        let range = okx_core::time_range_from_instants(range_start, range_end)
            .ok_or_else(|| VenueError::Decode("range_start must precede range_end".to_owned()))?;
        let path = format!(
            "/api/v5/market/history-candles?{}",
            okx_core::history_candles_query(&source_symbol, &interval, range)
        );
        let body = fetch(&path, 5).map_err(VenueError::Fetch)?;
        let candles = okx_core::parse_history_candles(&body, range).map_err(to_venue_error)?;
        Ok(candles
            .into_iter()
            .map(|candle| to_wit_bar(candle, spec))
            .collect())
    }
}

fn to_venue_error(error: okx_core::CoreError) -> VenueError {
    match error {
        okx_core::CoreError::Decode(message) => VenueError::Decode(message),
        okx_core::CoreError::Rejected(message) => VenueError::Rejected(message),
    }
}

fn to_wit_instrument(instrument: okx_core::SpotInstrument) -> Instrument {
    Instrument {
        symbol: instrument.symbol,
        source_symbol: instrument.source_symbol,
        name: instrument.name,
        base: instrument.base,
        quote: instrument.quote,
        // `okx_core::SpotInstrument` only ever describes a currently
        // tradable spot row (see `parse_spot_instruments`'s own docs), so
        // both are fixed here rather than threaded through: a suspended
        // row never reaches this function at all.
        kind: InstrumentKind::Spot,
        status: InstrumentStatus::Trading,
        price_scale: instrument.price_scale,
        tick_size: instrument.tick_size,
        qty_scale: instrument.qty_scale,
        step_size: instrument.step_size,
        contract: None,
    }
}

fn to_wit_bar_unit(unit: okx_core::BarUnit) -> BarUnit {
    match unit {
        okx_core::BarUnit::Second => BarUnit::Second,
        okx_core::BarUnit::Minute => BarUnit::Minute,
        okx_core::BarUnit::Hour => BarUnit::Hour,
        okx_core::BarUnit::Day => BarUnit::Day,
        okx_core::BarUnit::Week => BarUnit::Week,
        okx_core::BarUnit::Month => BarUnit::Month,
    }
}

fn from_wit_bar_unit(unit: BarUnit) -> okx_core::BarUnit {
    match unit {
        BarUnit::Second => okx_core::BarUnit::Second,
        BarUnit::Minute => okx_core::BarUnit::Minute,
        BarUnit::Hour => okx_core::BarUnit::Hour,
        BarUnit::Day => okx_core::BarUnit::Day,
        BarUnit::Week => okx_core::BarUnit::Week,
        BarUnit::Month => okx_core::BarUnit::Month,
    }
}

fn to_wit_bar_spec(spec: okx_core::BarSpec) -> BarSpec {
    BarSpec {
        step: spec.step,
        unit: to_wit_bar_unit(spec.unit),
    }
}

fn from_wit_bar_spec(spec: BarSpec) -> okx_core::BarSpec {
    okx_core::BarSpec {
        step: spec.step,
        unit: from_wit_bar_unit(spec.unit),
    }
}

fn to_wit_bar(candle: okx_core::CandleBar, spec: BarSpec) -> Bar {
    Bar {
        ts_open: candle.ts_open.as_nanos(),
        spec,
        open: Scaled {
            scale: candle.price_scale,
            value: candle.open,
        },
        high: Scaled {
            scale: candle.price_scale,
            value: candle.high,
        },
        low: Scaled {
            scale: candle.price_scale,
            value: candle.low,
        },
        close: Scaled {
            scale: candle.price_scale,
            value: candle.close,
        },
        volume: Volume::Real(Scaled {
            scale: candle.qty_scale,
            value: candle.volume,
        }),
        quote_volume: Some(Scaled {
            scale: candle.qty_scale,
            value: candle.quote_volume,
        }),
        // Neither reported by `/market/history-candles`.
        trade_count: None,
        taker_buy_volume: None,
    }
}

export_venue!(OkxVenue);
