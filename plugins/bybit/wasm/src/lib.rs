//! Bybit's `venue-plugin` component: spot instruments and bars, fetched
//! entirely through the host's `fetch` — this crate never imports
//! `wasi:sockets` or `wasi:http`, and has no way to (`wit/senken.wit`'s
//! `venue-plugin` world only ever imports `http`).
//!
//! Every byte this component decodes goes through `bybit-core`'s parsing —
//! the same parsing `plugins/bybit`'s native adapter calls for its own
//! spot source — so this file is a thin wrapper: build the request path,
//! call [`fetch`], hand the bytes to `bybit-core`, translate its neutral
//! result into the WIT wire types. See `bybit-core`'s own module docs for
//! the venue facts (row shape, the 1000-row kline cap, the closure check
//! against the response's own `time`) neither side hand-writes twice.
//!
//! Spot only, not linear/inverse/option: `wit/senken.wit`'s `instrument`
//! record can now carry a derivative's contract terms, but this component
//! has not been extended to serve them — its catalog is exactly what
//! `bybit_core::parse_spot_instruments` returns and nothing else.
//! `plugins/bybit`'s native adapter still serves the other markets and the
//! live feed — see that crate's own `Plugin::activate_with_http` for the
//! split.
//!
//! # Known gap: instrument-catalog pagination
//!
//! Bybit's spot catalog can span more than one page (`nextPageCursor`);
//! the native adapter follows it, up to 32 pages
//! (`plugins/bybit/src/lib.rs`'s own `paged`/`MAX_PAGES`). This component
//! fetches only the first page and refuses outright when the venue says
//! there is more, rather than serving a partial catalogue as a whole one —
//! the recorded fixture this porting wave's
//! equivalence test proves against (`plugins/bybit/tests/fixtures/spot.json`)
//! is itself a single page with an empty cursor, so there is no recorded
//! multi-page response to build or verify a follow-the-cursor loop
//! against without guessing its shape live, which this machine cannot do
//! (`AGENTS.md`: no live Bybit network access here). A real deployment
//! whose spot catalog exceeds one page would see the wasm-served catalog
//! silently narrower than native's until this is revisited with a
//! recorded multi-page fixture.

use senken_plugin_api::{
    Bar, BarSpec, BarUnit, Instrument, InstrumentKind, InstrumentStatus, Scaled, VenueDescriptor,
    VenueError, VenueGuest, Volume, export_venue, fetch,
};

struct BybitVenue;

impl VenueGuest for BybitVenue {
    fn descriptor() -> VenueDescriptor {
        VenueDescriptor {
            // The same source id the compiled-in adapter served spot under
            // before this component took that job (`SPOT_ID` in the native
            // crate, which its own bar-source test still asserts, and what
            // the live feed declares it serves). A saved chart layout stores
            // the instrument id it was drawn for, and the source is half of
            // that id — renaming it here orphans every one of them, with no
            // error anywhere to say so.
            id: "bybit-spot".to_owned(),
            name: "Bybit".to_owned(),
            base_url: "https://api.bybit.com".to_owned(),
        }
    }

    fn instruments() -> Result<Vec<Instrument>, VenueError> {
        // `limit=1000` copied from `plugins/bybit/src/lib.rs`'s own
        // `PAGE_LIMIT` — see this module's own docs on the pagination this
        // single fetch does not yet follow.
        let body = fetch("/v5/market/instruments-info?category=spot&limit=1000", 1)
            .map_err(VenueError::Fetch)?;
        // A first page that is the whole catalogue is the common case and
        // the only one this component can answer correctly. When the venue
        // says there is more, returning what arrived would hand back a
        // catalogue that looks complete and is not — a reader cannot tell a
        // venue with few pairs from one that was cut off. Refusing says so.
        if let Some(cursor) = bybit_core::spot_next_page_cursor(&body).map_err(to_venue_error)? {
            return Err(VenueError::Rejected(format!(
                "this venue's spot catalogue spans more than one page (cursor {cursor}); \
                 serving only the first page would hide the rest"
            )));
        }
        let instruments = bybit_core::parse_spot_instruments(&body).map_err(to_venue_error)?;
        Ok(instruments.into_iter().map(to_wit_instrument).collect())
    }

    fn supported_specs() -> Vec<BarSpec> {
        bybit_core::supported_bar_specs()
            .into_iter()
            .map(to_wit_bar_spec)
            .collect()
    }

    fn max_rows() -> u32 {
        bybit_core::KLINE_MAX_ROWS
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
        let interval = bybit_core::bybit_interval(core_spec)
            .ok_or_else(|| VenueError::Rejected(format!("unsupported bar spec {core_spec:?}")))?;
        let range = bybit_core::time_range_from_instants(range_start, range_end)
            .ok_or_else(|| VenueError::Decode("range_start must precede range_end".to_owned()))?;
        let path = format!(
            "/v5/market/kline?{}",
            bybit_core::kline_query(&source_symbol, &interval, range)
        );
        let body = fetch(&path, 5).map_err(VenueError::Fetch)?;
        let candles = bybit_core::parse_klines(&body, core_spec, range).map_err(to_venue_error)?;
        Ok(candles
            .into_iter()
            .map(|candle| to_wit_bar(candle, spec))
            .collect())
    }
}

fn to_venue_error(error: bybit_core::CoreError) -> VenueError {
    match error {
        bybit_core::CoreError::Decode(message) => VenueError::Decode(message),
        bybit_core::CoreError::Rejected(message) => VenueError::Rejected(message),
    }
}

fn to_wit_instrument(instrument: bybit_core::SpotInstrument) -> Instrument {
    Instrument {
        symbol: instrument.symbol,
        source_symbol: instrument.source_symbol,
        name: instrument.name,
        base: instrument.base,
        quote: instrument.quote,
        // `bybit_core::SpotInstrument` only ever describes a spot row (see
        // this crate's own module docs), so both are fixed here rather
        // than threaded through.
        kind: InstrumentKind::Spot,
        status: InstrumentStatus::Trading,
        price_scale: instrument.price_scale,
        tick_size: instrument.tick_size,
        qty_scale: instrument.qty_scale,
        step_size: instrument.step_size,
        contract: None,
    }
}

fn to_wit_bar_unit(unit: bybit_core::BarUnit) -> BarUnit {
    match unit {
        bybit_core::BarUnit::Second => BarUnit::Second,
        bybit_core::BarUnit::Minute => BarUnit::Minute,
        bybit_core::BarUnit::Hour => BarUnit::Hour,
        bybit_core::BarUnit::Day => BarUnit::Day,
        bybit_core::BarUnit::Week => BarUnit::Week,
        bybit_core::BarUnit::Month => BarUnit::Month,
    }
}

fn from_wit_bar_unit(unit: BarUnit) -> bybit_core::BarUnit {
    match unit {
        BarUnit::Second => bybit_core::BarUnit::Second,
        BarUnit::Minute => bybit_core::BarUnit::Minute,
        BarUnit::Hour => bybit_core::BarUnit::Hour,
        BarUnit::Day => bybit_core::BarUnit::Day,
        BarUnit::Week => bybit_core::BarUnit::Week,
        BarUnit::Month => bybit_core::BarUnit::Month,
    }
}

fn to_wit_bar_spec(spec: bybit_core::BarSpec) -> BarSpec {
    BarSpec {
        step: spec.step,
        unit: to_wit_bar_unit(spec.unit),
    }
}

fn from_wit_bar_spec(spec: BarSpec) -> bybit_core::BarSpec {
    bybit_core::BarSpec {
        step: spec.step,
        unit: from_wit_bar_unit(spec.unit),
    }
}

fn to_wit_bar(candle: bybit_core::CandleBar, spec: BarSpec) -> Bar {
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
        // Bybit never reports a trade count or a taker-buy split.
        trade_count: None,
        taker_buy_volume: None,
    }
}

export_venue!(BybitVenue);
