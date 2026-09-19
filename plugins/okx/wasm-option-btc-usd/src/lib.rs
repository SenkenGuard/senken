//! OKX's `BTC-USD` options `venue-plugin` component: the instrument
//! catalog for that family, fetched entirely through the host's `fetch` —
//! this crate never imports `wasi:sockets` or `wasi:http`, and has no way
//! to (`wit/senken.wit`'s `venue-plugin` world only ever imports `http`).
//!
//! Every byte this component decodes goes through `okx-core`'s parsing —
//! the same parsing `plugins/okx`'s native adapter calls for this exact
//! family, and the same one this crate's sibling components call for
//! theirs — so this file is a thin wrapper: build the request path, call
//! [`fetch`], hand the bytes to `okx-core`, translate its neutral result
//! into the WIT wire types.
//!
//! No bars: OKX options have never been wired for bar history in this
//! codebase's native adapter either —
//! `plugins/okx/src/lib.rs`'s own `activate_with_http` registers no
//! `BarSource` for any option family, only a `MarketDataSource`. This
//! component preserves that exact scope rather than guessing at an
//! endpoint shape nothing here has verified against a real response (see
//! `AGENTS.md`'s "do not invent venue facts").

use senken_plugin_api::{
    Bar, BarSpec, Contract, Instrument, InstrumentKind, InstrumentStatus, OptionRight, OptionTerms,
    Settlement, VenueDescriptor, VenueError, VenueGuest, export_venue, fetch,
};

struct OkxOptionBtcUsdVenue;

impl VenueGuest for OkxOptionBtcUsdVenue {
    fn descriptor() -> VenueDescriptor {
        VenueDescriptor {
            // The same source id the compiled-in adapter served this
            // option family under before this component took the job —
            // see this crate's own module docs for why that id cannot
            // change.
            id: "okx-option-btc-usd".to_owned(),
            name: "OKX BTC-USD Options".to_owned(),
            base_url: "https://www.okx.com".to_owned(),
        }
    }

    fn instruments() -> Result<Vec<Instrument>, VenueError> {
        let body = fetch(
            "/api/v5/public/instruments?instType=OPTION&instFamily=BTC-USD",
            1,
        )
        .map_err(VenueError::Fetch)?;
        let instruments =
            okx_core::parse_instruments(&body, okx_core::Market::Option).map_err(to_venue_error)?;
        Ok(instruments.into_iter().map(to_wit_instrument).collect())
    }

    fn supported_specs() -> Vec<BarSpec> {
        Vec::new()
    }

    fn max_rows() -> u32 {
        0
    }

    fn bars(
        _source_symbol: String,
        _spec: BarSpec,
        _range_start: i64,
        _range_end: i64,
    ) -> Result<Vec<Bar>, VenueError> {
        Err(VenueError::Rejected(
            "OKX option bar history is not served by this component".to_owned(),
        ))
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
        // computation. Named as a closure rather than a `senken_core::`
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

export_venue!(OkxOptionBtcUsdVenue);
