//! OKX's perpetual-swap `venue-plugin` component: swap instruments and
//! bars, fetched entirely through the host's `fetch` — this crate never
//! imports `wasi:sockets` or `wasi:http`, and has no way to (`wit/
//! senken.wit`'s `venue-plugin` world only ever imports `http`).
//!
//! Every byte this component decodes goes through `okx-core`'s parsing —
//! the same parsing `plugins/okx`'s native adapter calls for this exact
//! market, and the same one `plugins/okx/wasm` (spot) and this crate's
//! three siblings (futures, and the two option families) call for theirs —
//! so this file is a thin wrapper: build the request path, call [`fetch`],
//! hand the bytes to `okx-core`, translate its neutral result into the WIT
//! wire types. See `okx-core`'s own module docs for the venue facts a
//! caller on either side of the native/wasm boundary never spells out
//! twice.

use senken_plugin_api::{
    Bar, BarSpec, BarUnit, Contract, Instrument, InstrumentKind, InstrumentStatus, OptionRight,
    OptionTerms, Scaled, Settlement, VenueDescriptor, VenueError, VenueGuest, Volume, export_venue,
    fetch,
};

struct OkxSwapVenue;

impl VenueGuest for OkxSwapVenue {
    fn descriptor() -> VenueDescriptor {
        VenueDescriptor {
            // The same source id the compiled-in adapter served this
            // market under before this component took the job — see this
            // crate's own module docs for why that id cannot change.
            id: "okx-swap".to_owned(),
            name: "OKX Swap".to_owned(),
            base_url: "https://www.okx.com".to_owned(),
        }
    }

    fn instruments() -> Result<Vec<Instrument>, VenueError> {
        let body =
            fetch("/api/v5/public/instruments?instType=SWAP", 1).map_err(VenueError::Fetch)?;
        let instruments =
            okx_core::parse_instruments(&body, okx_core::Market::Swap).map_err(to_venue_error)?;
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

fn to_wit_kind(kind: okx_core::InstrumentKind) -> InstrumentKind {
    match kind {
        okx_core::InstrumentKind::Spot => InstrumentKind::Spot,
        okx_core::InstrumentKind::Future => InstrumentKind::Future,
        okx_core::InstrumentKind::Option => InstrumentKind::Option,
        okx_core::InstrumentKind::Perpetual => InstrumentKind::Perpetual,
    }
}

fn to_wit_status(status: okx_core::InstrumentStatus) -> InstrumentStatus {
    match status {
        okx_core::InstrumentStatus::Trading => InstrumentStatus::Trading,
        okx_core::InstrumentStatus::Halted => InstrumentStatus::Halted,
        okx_core::InstrumentStatus::PreOpen => InstrumentStatus::PreOpen,
        okx_core::InstrumentStatus::Closed => InstrumentStatus::Closed,
        okx_core::InstrumentStatus::Test => InstrumentStatus::Test,
        okx_core::InstrumentStatus::Unknown => InstrumentStatus::Unknown,
    }
}

fn to_wit_settlement(settlement: okx_core::Settlement) -> Settlement {
    match settlement {
        okx_core::Settlement::Linear => Settlement::Linear,
        okx_core::Settlement::Inverse => Settlement::Inverse,
        okx_core::Settlement::Quanto => Settlement::Quanto,
    }
}

fn to_wit_option_right(right: okx_core::OptionRight) -> OptionRight {
    match right {
        okx_core::OptionRight::Call => OptionRight::Call,
        okx_core::OptionRight::Put => OptionRight::Put,
    }
}

fn to_wit_contract(contract: okx_core::Contract) -> Contract {
    Contract {
        settle: contract.settle,
        settlement: to_wit_settlement(contract.settlement),
        // `wit/senken.wit`'s `instant` is already nanoseconds since the
        // epoch, the same unit `okx_core::Contract::expiry`'s
        // `senken_core::UnixNanos` stores — a plain field copy, not a
        // computation, the same reason
        // `senken_plugin_api::convert::instant_from_nanos` is the identity
        // function. Named as a closure rather than a `senken_core::`
        // path so this crate need not name `senken-core` as a direct
        // dependency just to call one inherent method.
        expiry: contract.expiry.map(|expiry| expiry.as_nanos()),
        size_scale: contract.size_scale,
        contract_size: contract.contract_size,
        option: contract.option.map(|option| OptionTerms {
            right: to_wit_option_right(option.right),
            strike_scale: option.strike_scale,
            strike: option.strike,
        }),
    }
}

fn to_wit_instrument(instrument: okx_core::Instrument) -> Instrument {
    Instrument {
        symbol: instrument.symbol,
        source_symbol: instrument.source_symbol,
        name: instrument.name,
        base: instrument.base,
        quote: instrument.quote,
        kind: to_wit_kind(instrument.kind),
        status: to_wit_status(instrument.status),
        price_scale: instrument.price_scale,
        tick_size: instrument.tick_size,
        qty_scale: instrument.qty_scale,
        step_size: instrument.step_size,
        contract: instrument.contract.map(to_wit_contract),
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

export_venue!(OkxSwapVenue);
