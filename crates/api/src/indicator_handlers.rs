//! Indicators over HTTP.
//!
//! `GET /api/indicators` lists the ten built-ins `senken-indicators`
//! implements, plus every enabled indicator loaded from an uploaded
//! `.wasm` component (`senken_runtime::DynamicIndicators`); `POST
//! /api/indicators/compute` evaluates one of them over a bar range already
//! resolvable through `SeriesLoader::resolve` (the same read path
//! `bars_handlers::range_bars` uses — this endpoint fetches nothing
//! itself, so a caller must have already `POST /api/bars/ensure`d the
//! range it asks about). Building and reading the concrete indicator
//! reuses `senken_indicators::ConcreteIndicator` for a built-in — the
//! dynamic build-and-read contract every consumer of the ten built-ins
//! shares (an alert row, a workspace layer, the live indicator sessions
//! `ws` drives) — rather than a second factory: one source of truth for the
//! maths, not the browser's own copy. A name this crate does not recognise
//! as one of the ten is looked up in `DynamicIndicators` next, so a client
//! resolves exactly one catalogue rather than needing to know in advance
//! which of the two implemented it.
//!
//! `POST /api/indicators/plugins`, `GET /api/indicators/plugins` and `POST
//! /api/indicators/plugins/{name}/enabled` manage that second source: they
//! require `Action::Create`/`Action::View`/`Action::Edit` on
//! `Resource::Indicator` at `Scope::All` — the same "not owned by any one
//! account" shape `storage_handlers` uses for disk usage, since a loaded
//! `.wasm` component runs for every user of this server, not one.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};

use senken_acl::{Action, Resource, Scope};
use senken_alerts::IndicatorSpec;
use senken_core::TimeRange;
use senken_identity::AuthenticatedUser;
use senken_indicator_lang::CompileError;
use senken_indicators::{
    ConcreteIndicator, DESCRIPTORS, DisplayList, Drawable, Extend, IndicatorDescriptor,
    IndicatorField, LabelAnchor, ParamDefault, ParamKind, Placement, PlotShape, Point, PriceCoord,
    ScaleHint, ScaledPrice, SeriesShape, VolumeRequirement, descriptor,
};
use senken_runtime::{
    DynamicIndicatorError, DynamicIndicatorInfo, DynamicIndicatorStatus, reject_if_over_display_cap,
};
use senken_series::{BarSpec, Origin};

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    BarRangeQuery, CompileIndicatorErrorDto, CompileIndicatorRequest, ComputeIndicatorRequest,
    ComputeIndicatorResponse, IndicatorCatalogEntry, IndicatorDrawableDto,
    IndicatorDrawablePointDto, IndicatorExtendDto, IndicatorLabelAnchorDto,
    IndicatorParamDefaultDto, IndicatorParamDto, IndicatorPlacementDto, IndicatorPlotDto,
    IndicatorPluginDto, IndicatorPointDto, IndicatorPriceCoordDto, IndicatorScaleDto,
    IndicatorScaledPriceDto, SetIndicatorPluginEnabledRequest,
};

/// Requires `action` on `Resource::Indicator`, and specifically at
/// `Scope::All` — the same reasoning
/// `storage_handlers::require_storage_all` states for disk usage: an
/// uploaded `.wasm` component is a property of this server's whole plugin
/// population, not any one account, so `Scope::Own` means nothing here
/// either.
fn require_indicator_plugins_all(
    user: &AuthenticatedUser,
    action: Action,
) -> Result<(), HandlerError> {
    let scope = user.authorize(action, Resource::Indicator)?;
    if scope == Scope::All {
        Ok(())
    } else {
        Err(HandlerError::Forbidden(
            "you do not have permission to do that".to_owned(),
        ))
    }
}

impl From<DynamicIndicatorError> for HandlerError {
    fn from(error: DynamicIndicatorError) -> Self {
        Self::BadRequest(error.to_string())
    }
}

/// [`entry`]'s counterpart for a [`DynamicIndicatorInfo`] — the same wire
/// shape, with the defaults `wit/senken.wit`'s `indicator-descriptor` gives
/// a caller no way to declare: no `scale` hint (`Own`, the same "defines
/// its own scale" case a built-in with a genuinely indicator-specific range
/// would use), no volume requirement (`false`/`Any`, the least restrictive
/// choice), no `placement` preference (`Either`), and no smoothing model to
/// derive a warm-up depth from (`0` — a dynamic indicator's own
/// `initialized()` still gates which points a client ever sees; it simply
/// is not pre-warmed with extra bars before the requested range the way a
/// built-in is).
fn dynamic_entry(info: &DynamicIndicatorInfo) -> IndicatorCatalogEntry {
    IndicatorCatalogEntry {
        name: info.id.clone(),
        title: info.title.clone(),
        short_title: info.short_title.clone(),
        legend: info.legend.clone(),
        params: info
            .params
            .iter()
            .map(|param| IndicatorParamDto {
                name: param.name.clone(),
                kind: match param.kind {
                    ParamKind::Integer => "integer",
                    ParamKind::Number => "number",
                }
                .to_owned(),
                default: match param.default {
                    ParamDefault::Integer(value) => IndicatorParamDefaultDto::Integer(value),
                    ParamDefault::Number(value) => IndicatorParamDefaultDto::Number(value),
                },
                min: param.min,
            })
            .collect(),
        plots: info
            .plots
            .iter()
            .map(|plot| IndicatorPlotDto {
                field: plot.field.clone(),
                label: plot.label.clone(),
                shape: match plot.shape {
                    PlotShape::Line => "line",
                    PlotShape::Histogram => "histogram",
                }
                .to_owned(),
                color: plot.color.clone(),
            })
            .collect(),
        scale: IndicatorScaleDto::Own,
        requires_real_volume: false,
        placement: IndicatorPlacementDto::Either,
        warmup_bars: 0,
    }
}

/// [`DynamicIndicatorStatus`]'s wire form for `GET /api/indicators/plugins`
/// — see [`crate::dto::IndicatorPluginDto`]'s own docs. `entry` comes from
/// [`dynamic_entry`] when `status.info` is present, and is `None` for a
/// registration that never finished loading (`Incompatible`/
/// `FailedToLoad`), which never produced one.
fn indicator_plugin_dto(status: DynamicIndicatorStatus) -> IndicatorPluginDto {
    IndicatorPluginDto {
        id: status.id,
        entry: status.info.as_ref().map(dynamic_entry),
        origin: status.origin.into(),
        state: status.state.into(),
        health: status.health.map(Into::into),
        logs: status.logs.into_iter().map(Into::into).collect(),
    }
}

fn entry(descriptor: &IndicatorDescriptor) -> IndicatorCatalogEntry {
    IndicatorCatalogEntry {
        name: descriptor.id.to_owned(),
        title: descriptor.title.to_owned(),
        short_title: descriptor.short_title.to_owned(),
        legend: descriptor.legend.to_owned(),
        params: descriptor
            .params
            .iter()
            .map(|param| IndicatorParamDto {
                name: param.name.to_owned(),
                kind: match param.kind {
                    ParamKind::Integer => "integer",
                    ParamKind::Number => "number",
                }
                .to_owned(),
                default: match param.default {
                    ParamDefault::Integer(value) => IndicatorParamDefaultDto::Integer(value),
                    ParamDefault::Number(value) => IndicatorParamDefaultDto::Number(value),
                },
                min: param.min,
            })
            .collect(),
        plots: descriptor
            .plots
            .iter()
            .map(|plot| IndicatorPlotDto {
                field: plot.field.to_owned(),
                label: plot.label.to_owned(),
                shape: match plot.shape {
                    PlotShape::Line => "line",
                    PlotShape::Histogram => "histogram",
                }
                .to_owned(),
                color: plot.color.to_owned(),
            })
            .collect(),
        scale: match descriptor.scale {
            ScaleHint::Price => IndicatorScaleDto::Price,
            ScaleHint::Ratio { min, max } => IndicatorScaleDto::Ratio { min, max },
            ScaleHint::Volume => IndicatorScaleDto::Volume,
            ScaleHint::Own => IndicatorScaleDto::Own,
        },
        requires_real_volume: matches!(descriptor.requires, VolumeRequirement::Real),
        placement: match descriptor.placement {
            Placement::Overlay => IndicatorPlacementDto::Overlay,
            Placement::SubPane => IndicatorPlacementDto::SubPane,
            Placement::Either => IndicatorPlacementDto::Either,
        },
        warmup_bars: descriptor.warmup_bars(|_| None),
    }
}

/// The wire name for one [`IndicatorField`] — [`IndicatorField::wire_name`]
/// itself, so a client sees one vocabulary for "which number does this
/// indicator report" everywhere it appears (an alert's stored
/// `condition_field` column, this endpoint's response, a live indicator
/// session's WS frame).
fn field_key(field: IndicatorField) -> &'static str {
    field.wire_name()
}

/// Extends `range` leftward by whatever `descriptor.warmup_bars` says this
/// indicator (with `params`) needs before it can be trusted from
/// `range`'s own start — the same prefix `compute_indicator` has always
/// resolved, factored out so `ws::subscribe`'s live indicator sessions
/// warm up over the exact same rule instead of a second copy of it.
///
/// # Errors
/// [`HandlerError::BadRequest`] if `params` is not valid JSON, or if the
/// extended range cannot be represented ([`TimeRange::new`] rejects an
/// inverted range).
pub(crate) fn warmup_extended_range(
    descriptor: &IndicatorDescriptor,
    spec: BarSpec,
    params: &str,
    range: senken_core::TimeRange,
) -> Result<senken_core::TimeRange, HandlerError> {
    let parameter_values: serde_json::Value = serde_json::from_str(params)
        .map_err(|error| HandlerError::BadRequest(error.to_string()))?;
    let warmup_bars = descriptor.warmup_bars(|name| {
        parameter_values
            .get(name)
            .and_then(serde_json::Value::as_u64)
    });
    let prefix_start = spec
        .duration_nanos()
        .and_then(|duration| {
            i64::try_from(warmup_bars)
                .ok()
                .and_then(|bars| bars.checked_mul(duration))
                .and_then(|prefix| range.start().as_nanos().checked_sub(prefix))
        })
        .unwrap_or(range.start().as_nanos());
    TimeRange::new(
        senken_core::UnixNanos::from_nanos(prefix_start),
        range.end(),
    )
    .ok_or(HandlerError::BadRequest(
        "invalid indicator range".to_owned(),
    ))
}

fn display_for_indicator(
    indicator: &mut ConcreteIndicator,
    descriptor: &IndicatorDescriptor,
    bars: &[senken_series::Bar],
    provisional: Option<senken_series::Bar>,
    start: senken_core::UnixNanos,
) -> (Vec<IndicatorDrawableDto>, usize) {
    let mut fields: Vec<(IndicatorField, Vec<IndicatorDrawablePointDto>)> = indicator
        .reported_fields()
        .iter()
        .copied()
        .map(|field| (field, Vec::new()))
        .collect();
    for bar in bars.iter().chain(provisional.iter()) {
        indicator.handle_bar(bar);
        if bar.ts_open < start || !indicator.initialized() {
            continue;
        }
        for (field, points) in &mut fields {
            if let Ok(value) = indicator.read(*field) {
                points.push(IndicatorDrawablePointDto {
                    ts_open: bar.ts_open.as_nanos(),
                    value,
                });
            }
        }
    }

    let mut display = DisplayList::new(descriptor.max_display_objects);
    for (field, points) in fields {
        let shape = descriptor
            .plots
            .iter()
            .find(|plot| plot.field == field_key(field))
            .map_or(PlotShape::Line, |plot| plot.shape);
        display.push(Drawable::Series {
            field: field_key(field).to_owned(),
            shape: match shape {
                PlotShape::Line => SeriesShape::Line,
                PlotShape::Histogram => SeriesShape::Histogram,
            },
            points: points
                .into_iter()
                .map(|point| Point {
                    time: point.ts_open,
                    value: point.value,
                })
                .collect(),
        });
    }
    let discarded_objects = display.discarded_objects();
    let display = display.drawables().map(drawable_dto).collect();
    (display, discarded_objects)
}

/// Converts one `senken_indicators::Drawable` into its wire form.
///
/// Every variant is matched explicitly and on purpose: a display list built
/// from a wildcard arm would silently drop any indicator's non-series
/// geometry (a zone, a pivot label) before it ever reached a client.
fn drawable_dto(drawable: &Drawable) -> IndicatorDrawableDto {
    match drawable {
        Drawable::Series {
            field,
            shape,
            points,
        } => IndicatorDrawableDto::Series {
            field: field.clone(),
            shape: series_shape_key(*shape).to_owned(),
            points: points
                .iter()
                .map(|point| IndicatorDrawablePointDto {
                    ts_open: point.time,
                    value: point.value,
                })
                .collect(),
        },
        Drawable::Segment { a, b, extend } => IndicatorDrawableDto::Segment {
            a: point_dto(*a),
            b: point_dto(*b),
            extend: extend_dto(*extend),
        },
        Drawable::Level { price, extend } => IndicatorDrawableDto::Level {
            price: price_coord_dto(*price),
            extend: extend_dto(*extend),
        },
        Drawable::Box { a, b } => IndicatorDrawableDto::Box {
            a: point_dto(*a),
            b: point_dto(*b),
        },
        Drawable::Label { at, text, anchor } => IndicatorDrawableDto::Label {
            at: point_dto(*at),
            text: text.clone(),
            anchor: label_anchor_dto(*anchor),
        },
    }
}

fn series_shape_key(shape: SeriesShape) -> &'static str {
    match shape {
        SeriesShape::Line => "line",
        SeriesShape::Histogram => "histogram",
        SeriesShape::Area => "area",
    }
}

fn point_dto(point: Point) -> IndicatorPointDto {
    IndicatorPointDto {
        time: point.time,
        value: point.value,
    }
}

fn extend_dto(extend: Extend) -> IndicatorExtendDto {
    match extend {
        Extend::None => IndicatorExtendDto::None,
        Extend::Forward => IndicatorExtendDto::Forward,
        Extend::Backward => IndicatorExtendDto::Backward,
        Extend::Both => IndicatorExtendDto::Both,
    }
}

fn label_anchor_dto(anchor: LabelAnchor) -> IndicatorLabelAnchorDto {
    match anchor {
        LabelAnchor::Above => IndicatorLabelAnchorDto::Above,
        LabelAnchor::Below => IndicatorLabelAnchorDto::Below,
        LabelAnchor::Center => IndicatorLabelAnchorDto::Center,
    }
}

fn price_coord_dto(price: PriceCoord) -> IndicatorPriceCoordDto {
    match price {
        PriceCoord::Annotation(value) => IndicatorPriceCoordDto::Annotation(value),
        PriceCoord::Executable(scaled) => {
            IndicatorPriceCoordDto::Executable(scaled_price_dto(scaled))
        }
    }
}

fn scaled_price_dto(scaled: ScaledPrice) -> IndicatorScaledPriceDto {
    IndicatorScaledPriceDto {
        value: scaled.value,
        scale: scaled.scale,
    }
}

/// `GET /api/indicators`: the catalogue of `senken-indicators`' ten
/// built-ins, plus every currently-enabled indicator loaded from an
/// uploaded `.wasm` component. A plugin disabled through `POST
/// /api/indicators/plugins/{name}/enabled` drops out of this list
/// immediately — see `senken_runtime::DynamicIndicators::catalog`'s own
/// docs for why a chart already showing it is left to notice on its own
/// (a placeholder, keeping its stored parameters) rather than this
/// endpoint reaching into any chart's state.
#[utoipa::path(
    get,
    path = "/api/indicators",
    responses(
        (status = 200, body = Vec<IndicatorCatalogEntry>),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_indicators(
    State(state): State<AppState>,
    Extension(_ctx): Authed,
) -> Json<Vec<IndicatorCatalogEntry>> {
    let mut entries: Vec<IndicatorCatalogEntry> = DESCRIPTORS.iter().map(entry).collect();
    entries.extend(
        state
            .runtime
            .dynamic_indicators()
            .catalog()
            .iter()
            .map(dynamic_entry),
    );
    Json(entries)
}

/// `POST /api/indicators/compute`: replays
/// whatever bars are already resolvable for `instrument`/`spec`/`from`/`to`
/// through the named indicator, one bar at a time — the same incremental
/// discipline `senken-indicators` itself is built on ("one code path, live or backfilled") — and reports one point per bar once the
/// indicator is `initialized()` (never a warm-up value). `indicator.name`
/// is looked up against the ten built-ins first and `DynamicIndicators`
/// second, so a client never needs to know in advance which catalogue
/// serves a given name.
#[utoipa::path(
    post,
    path = "/api/indicators/compute",
    request_body = ComputeIndicatorRequest,
    responses(
        (status = 200, body = ComputeIndicatorResponse),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn compute_indicator(
    State(state): State<AppState>,
    Extension(_ctx): Authed,
    Json(body): Json<ComputeIndicatorRequest>,
) -> Result<Json<ComputeIndicatorResponse>, HandlerError> {
    let provisional = body.provisional;
    let indicator = body.indicator;
    let query = BarRangeQuery {
        instrument: body.instrument,
        spec: body.spec,
        from: body.from,
        to: body.to,
    };
    let (id, spec, _hit) =
        crate::bars_handlers::resolve_bar_target(&state, &query.instrument, &query.spec).await?;
    let loader = crate::bars_handlers::loader_for(&state, &id)?;
    let range = crate::bars_handlers::parse_range(query.from, query.to)?;

    // The forming bar the client is drawing, folded in last so the newest
    // indicator point lands on the same bar as the newest candle. It is not
    // stored and never will be in this shape: the next reload replaces it
    // with the bar the venue actually recorded.
    let provisional = provisional.map(|dto| senken_series::Bar {
        ts_open: senken_core::UnixNanos::from_nanos(dto.ts_open),
        open: dto.open,
        high: dto.high,
        low: dto.low,
        close: dto.close,
        volume: dto.volume.into(),
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    });

    let Some(descriptor) = descriptor(&indicator.name) else {
        return compute_dynamic_indicator(&state, &loader, id, spec, range, provisional, indicator)
            .await;
    };

    let resolve_range = warmup_extended_range(descriptor, spec, &indicator.params, range)?;
    let key = senken_series::SeriesKey::new(id.source(), id.symbol(), Origin::Derived, spec);
    let resolved = loader
        .resolve(&key, resolve_range, senken_series::Anchor::UTC)
        .await
        .map_err(|source| {
            tracing::error!(%source, "bars resolve failed while computing an indicator");
            HandlerError::Internal
        })?;

    let indicator_spec = IndicatorSpec::from(indicator);
    let mut indicator = indicator_spec
        .build()
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;

    let warmup_truncated = resolve_range.start() != range.start()
        && resolved
            .missing
            .iter()
            .any(|missing| missing.start() < range.start());
    let (display, discarded_objects) = display_for_indicator(
        &mut indicator,
        descriptor,
        &resolved.bars,
        provisional,
        range.start(),
    );

    Ok(Json(ComputeIndicatorResponse {
        display,
        discarded_objects,
        missing: resolved.missing.into_iter().map(Into::into).collect(),
        warmup_truncated,
    }))
}

/// [`compute_indicator`]'s dynamic-catalogue path: `descriptor(name)` found
/// nothing, so `name` is looked up in `DynamicIndicators` instead.
///
/// Unlike a built-in, a dynamic indicator declares no smoothing model
/// (`wit/senken.wit`'s `indicator-descriptor` has no field for one), so no
/// warm-up prefix is requested — `resolved.missing`/`warmup_truncated`
/// therefore always describe exactly `range`, never a prefix of it.
///
/// Every non-series display object produced across the whole range is
/// counted against `senken_runtime::DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS`;
/// exceeding it rejects the whole request with a message rather than
/// silently truncating the way a built-in's own `discarded_objects` count
/// does — see `senken_runtime::reject_if_over_display_cap`'s own docs for
/// why an untrusted plugin gets the stronger failure mode.
async fn compute_dynamic_indicator(
    state: &AppState,
    loader: &senken_loader::SeriesLoader,
    id: senken_marketdata::InstrumentId,
    spec: BarSpec,
    range: senken_core::TimeRange,
    provisional: Option<senken_series::Bar>,
    indicator: crate::dto::IndicatorSpecDto,
) -> Result<Json<ComputeIndicatorResponse>, HandlerError> {
    let mut instance = state
        .runtime
        .dynamic_indicators()
        .spawn(&indicator.name, &indicator.params)?;

    let key = senken_series::SeriesKey::new(id.source(), id.symbol(), Origin::Derived, spec);
    let resolved = loader
        .resolve(&key, range, senken_series::Anchor::UTC)
        .await
        .map_err(|source| {
            tracing::error!(%source, "bars resolve failed while computing a dynamic indicator");
            HandlerError::Internal
        })?;

    let mut fields: std::collections::BTreeMap<String, Vec<IndicatorDrawablePointDto>> =
        std::collections::BTreeMap::new();
    let mut display = DisplayList::new(senken_runtime::DYNAMIC_INDICATOR_MAX_DISPLAY_OBJECTS);
    for bar in resolved.bars.iter().chain(provisional.iter()) {
        let on_bar = instance.handle_bar(bar, spec)?;
        if bar.ts_open < range.start() || !instance.initialized()? {
            continue;
        }
        for (field, value) in on_bar.plots {
            fields
                .entry(field)
                .or_default()
                .push(IndicatorDrawablePointDto {
                    ts_open: bar.ts_open.as_nanos(),
                    value,
                });
        }
        for drawable in on_bar.drawables {
            display.push(drawable);
        }
    }
    reject_if_over_display_cap(
        &indicator.name,
        display.drawables().count(),
        display.discarded_objects(),
    )?;

    for (field, points) in fields {
        display.push(Drawable::Series {
            field,
            shape: SeriesShape::Line,
            points: points
                .into_iter()
                .map(|point| Point {
                    time: point.ts_open,
                    value: point.value,
                })
                .collect(),
        });
    }

    Ok(Json(ComputeIndicatorResponse {
        display: display.drawables().map(drawable_dto).collect(),
        discarded_objects: 0,
        missing: resolved.missing.into_iter().map(Into::into).collect(),
        warmup_truncated: false,
    }))
}

/// The body-size ceiling `crate::mount_indicator_routes` applies to `POST
/// /api/indicators/plugins`, in place of the router-wide default (sized for
/// JSON, not a compiled binary artifact) — see that mount call's own doc
/// comment for why.
pub(crate) const INDICATOR_PLUGIN_MAX_BYTES: usize = 16 * 1024 * 1024;

/// `POST /api/indicators/plugins`: registers a compiled `wasm32-wasip2`
/// component as a dynamic indicator, from the raw component bytes as the
/// request body. Requires `Action::Create` on `Resource::Indicator` at
/// `Scope::All`.
#[utoipa::path(
    post,
    path = "/api/indicators/plugins",
    request_body(content = Vec<u8>, content_type = "application/wasm"),
    responses(
        (status = 200, body = IndicatorCatalogEntry),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn upload_indicator_plugin(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    body: axum::body::Bytes,
) -> Result<Json<IndicatorCatalogEntry>, HandlerError> {
    require_indicator_plugins_all(&ctx.user, Action::Create)?;
    let info = state.runtime.dynamic_indicators().register(&body)?;
    Ok(Json(dynamic_entry(&info)))
}

/// `GET /api/indicators/plugins`: every registered dynamic indicator,
/// enabled or not, **including one that never finished loading** — unlike
/// `GET /api/indicators`, which only ever lists what a chart may place
/// right now. Each entry carries its own runtime health and ring log, so
/// this is also the one HTTP surface for diagnosing why a plugin is
/// `incompatible`, `failed_to_load` or `auto_disabled` — no separate
/// per-plugin log/health endpoint exists, since every entry already reports
/// both here. Requires `Action::View` on `Resource::Indicator` at
/// `Scope::All`.
#[utoipa::path(
    get,
    path = "/api/indicators/plugins",
    responses(
        (status = 200, body = Vec<IndicatorPluginDto>),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_indicator_plugins(
    State(state): State<AppState>,
    Extension(ctx): Authed,
) -> Result<Json<Vec<IndicatorPluginDto>>, HandlerError> {
    require_indicator_plugins_all(&ctx.user, Action::View)?;
    let plugins = state
        .runtime
        .dynamic_indicators()
        .all()
        .into_iter()
        .map(indicator_plugin_dto)
        .collect();
    Ok(Json(plugins))
}

/// `POST /api/indicators/plugins/{name}/enabled`: flips whether `GET
/// /api/indicators` currently offers this dynamic indicator, without
/// discarding the loaded component. Requires `Action::Edit` on
/// `Resource::Indicator` at `Scope::All`.
#[utoipa::path(
    post,
    path = "/api/indicators/plugins/{name}/enabled",
    params(("name" = String, Path)),
    request_body = SetIndicatorPluginEnabledRequest,
    responses(
        (status = 200),
        (status = 400, body = crate::dto::ErrorBody),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn set_indicator_plugin_enabled(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    axum::extract::Path(name): axum::extract::Path<String>,
    Json(body): Json<SetIndicatorPluginEnabledRequest>,
) -> Result<(), HandlerError> {
    require_indicator_plugins_all(&ctx.user, Action::Edit)?;
    state
        .runtime
        .dynamic_indicators()
        .set_enabled(&name, body.enabled)?;
    Ok(())
}

/// `POST /api/indicators/compile`'s failure, kept distinct from
/// [`HandlerError`] because [`CompileError::Syntax`]/[`CompileError::Type`]
/// must reach the authoring panel as the exact line, column and message the
/// compiler produced — the crate-wide [`crate::dto::ErrorBody`] has no
/// field for either, and flattening them into its one `error` string would
/// force the panel to re-parse prose to find the line it should highlight.
pub(crate) enum CompileIndicatorRejection {
    /// Authorisation failed, or the compiled component was rejected while
    /// registering — both already have a [`HandlerError`] shape.
    Handler(HandlerError),
    /// A mistake in the trader's own source.
    Compile(CompileError),
}

impl From<HandlerError> for CompileIndicatorRejection {
    fn from(error: HandlerError) -> Self {
        Self::Handler(error)
    }
}

impl From<CompileError> for CompileIndicatorRejection {
    fn from(error: CompileError) -> Self {
        Self::Compile(error)
    }
}

impl IntoResponse for CompileIndicatorRejection {
    fn into_response(self) -> Response {
        match self {
            Self::Handler(error) => error.into_response(),
            Self::Compile(
                CompileError::Syntax {
                    line,
                    column,
                    message,
                }
                | CompileError::Type {
                    line,
                    column,
                    message,
                },
            ) => (
                StatusCode::BAD_REQUEST,
                Json(CompileIndicatorErrorDto {
                    line,
                    column,
                    message,
                }),
            )
                .into_response(),
            // A bug in this compiler, not in anything the trader wrote —
            // reported like any other internal failure (logged here,
            // detail withheld from the client) rather than pointing at a
            // line this source never had. `CompileError` is
            // `#[non_exhaustive]`, so a variant added later without a line
            // and column (nothing else this crate knows how to present)
            // falls into the same arm as `Internal` rather than failing to
            // compile.
            Self::Compile(CompileError::Internal(message)) => {
                tracing::error!(
                    error = %message,
                    "indicator-lang: internal compiler error"
                );
                HandlerError::Internal.into_response()
            }
            Self::Compile(other) => {
                tracing::error!(
                    error = %other,
                    "indicator-lang: unrecognised compile error variant"
                );
                HandlerError::Internal.into_response()
            }
        }
    }
}

/// `POST /api/indicators/compile`: compiles indicator-lang `source` into a
/// component and registers it the same way `POST /api/indicators/plugins`
/// registers an uploaded one — the authoring panel's "run" action. Requires
/// `Action::Create` on `Resource::Indicator` at `Scope::All`, the same as
/// uploading a compiled component directly: either way the result joins
/// the one dynamic-indicator catalogue every user of this server shares.
#[utoipa::path(
    post,
    path = "/api/indicators/compile",
    request_body = CompileIndicatorRequest,
    responses(
        (status = 200, body = IndicatorCatalogEntry),
        (status = 400, description = "a mistake in the source, or the compiled component was rejected", body = CompileIndicatorErrorDto),
        (status = 401, body = crate::dto::ErrorBody),
        (status = 403, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn compile_indicator(
    State(state): State<AppState>,
    Extension(ctx): Authed,
    Json(body): Json<CompileIndicatorRequest>,
) -> Result<Json<IndicatorCatalogEntry>, CompileIndicatorRejection> {
    require_indicator_plugins_all(&ctx.user, Action::Create)?;
    let wasm = senken_indicator_lang::compile(&body.source)?;
    let info = state
        .runtime
        .dynamic_indicators()
        .register(&wasm)
        .map_err(HandlerError::from)?;
    Ok(Json(dynamic_entry(&info)))
}

#[cfg(test)]
mod tests {
    use senken_identity::DEFAULT_ADMIN_EMAIL;
    use senken_indicators::ConcreteIndicator;

    use crate::dto::IndicatorDrawableDto;

    use crate::bars_handlers::test_support::{runtime_with_fake_venue, test_instrument};
    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_bytes_auth, post_json, post_json_auth,
        serve_unfenced_test_server_with,
    };

    async fn admin_token(addr: std::net::SocketAddr) -> String {
        let response = post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": DEFAULT_ADMIN_EMAIL, "password": ADMIN_TEST_PASSWORD }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Builds one of `senken-runtime`'s own dynamic-indicator test fixtures
    /// (`crates/runtime/tests/fixtures/{name}`) to a real `wasm32-wasip2`
    /// component, the same way `senken_runtime`'s own `dynamic_indicators.rs`
    /// integration test does — reused rather than duplicated, since a
    /// fixture that proves the bridge already proves it for this crate's own
    /// HTTP surface over the exact same bytes. Serialized behind one
    /// process-wide lock for the same reason `senken-plugin-host`'s own
    /// fixture builder is: this machine must never run two Rust builds at
    /// once, and the test harness would otherwise start one `cargo build`
    /// per fixture-dependent test at the same time.
    fn build_dynamic_indicator_fixture(name: &str) -> Vec<u8> {
        static BUILD_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = BUILD_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let fixture_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../runtime/tests/fixtures")
            .join(name);
        let status = std::process::Command::new(env!("CARGO"))
            .args(["build", "--target", "wasm32-wasip2"])
            .current_dir(&fixture_dir)
            .status()
            .expect("spawning `cargo build` for a test fixture must succeed");
        assert!(status.success(), "fixture `{name}` failed to build");

        let binary_name = format!("fixture_{}.wasm", name.replace('-', "_"));
        let wasm_path = fixture_dir
            .join("target/wasm32-wasip2/debug")
            .join(&binary_name);
        std::fs::read(&wasm_path)
            .unwrap_or_else(|error| panic!("reading {}: {error}", wasm_path.display()))
    }

    #[tokio::test]
    async fn the_catalogue_lists_all_ten_built_ins_and_every_name_builds() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let response = get_auth(format!("http://{addr}/api/indicators"), &token).await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        let rows = body.as_array().unwrap();
        assert_eq!(rows.len(), 10, "the ten built-ins, no more, no fewer");

        for row in rows {
            let name = row["name"].as_str().unwrap();
            let params = row["params"].as_array().unwrap();
            // A minimal, always-valid JSON payload built from the
            // catalogue's own declared parameter names — proves the
            // catalogue and `ConcreteIndicator::build` cannot silently
            // drift apart.
            let json = serde_json::Value::Object(
                params
                    .iter()
                    .map(|p| (p["name"].as_str().unwrap().to_owned(), serde_json::json!(5)))
                    .collect(),
            );
            ConcreteIndicator::build(name, &json.to_string())
                .unwrap_or_else(|e| panic!("catalogue entry {name:?} does not build: {e}"));
        }

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn computing_an_sma_over_a_flat_series_reports_the_flat_value_once_warmed_up() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;
        let instrument = test_instrument();

        let range_to: i64 = 20 * 60 * 1_000_000_000; // 20 one-minute bars
        let ensure = post_json_auth(
            format!("http://{addr}/api/bars/ensure"),
            &token,
            serde_json::json!({ "instrument": instrument, "spec": "1m", "from": 0, "to": range_to }),
        )
        .await;
        let job_id = body_json(ensure).await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();

        // Poll the same job-status endpoint `bars_handlers`'s own tests use.
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let response =
                    get_auth(format!("http://{addr}/api/bars/jobs/{job_id}"), &token).await;
                if body_json(response).await["phase"] == "done" {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        let response = post_json_auth(
            format!("http://{addr}/api/indicators/compute"),
            &token,
            serde_json::json!({
                "instrument": instrument,
                "spec": "1m",
                "from": 0,
                "to": range_to,
                "indicator": { "name": "Sma", "params": r#"{"period":5}"# },
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        let display = body["display"].as_array().unwrap();
        assert_eq!(display.len(), 1, "SMA has one drawable series");
        let points = display[0]["points"].as_array().unwrap();
        // The fake venue's bars all close at 100 (see
        // `bars_handlers::test_support::m1_bars_for`), so a fully warmed-up
        // SMA reports exactly 100 — and only from the 5th bar onward.
        assert_eq!(
            points.len(),
            20 - 4,
            "the first period-1 bars report no value yet"
        );
        assert_eq!(display[0]["kind"], "series");
        assert_eq!(display[0]["field"], "value");
        for point in points {
            assert!((point["value"].as_f64().unwrap() - 100.0).abs() < f64::EPSILON);
        }

        handle.shutdown().await.unwrap();
    }

    /// An indicator's newest point must land on the bar the chart is
    /// actually drawing. The forming bar is not stored anywhere — the client
    /// assembles it from live ticks — so without sending it the line stops
    /// one bar short of the candles, which is what a reader notices first.
    #[tokio::test]
    async fn a_provisional_bar_extends_the_series_onto_the_forming_candle() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;
        let instrument = test_instrument();

        let range_to: i64 = 20 * 60 * 1_000_000_000;
        let ensure = post_json_auth(
            format!("http://{addr}/api/bars/ensure"),
            &token,
            serde_json::json!({ "instrument": instrument, "spec": "1m", "from": 0, "to": range_to }),
        )
        .await;
        let job_id = body_json(ensure).await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let response =
                    get_auth(format!("http://{addr}/api/bars/jobs/{job_id}"), &token).await;
                if body_json(response).await["phase"] == "done" {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();

        let compute = |provisional: Option<serde_json::Value>| {
            let token = token.clone();
            let instrument = instrument.clone();
            async move {
                let mut body = serde_json::json!({
                    "instrument": instrument,
                    "spec": "1m",
                    "from": 0,
                    "to": range_to,
                    "indicator": { "name": "Sma", "params": r#"{"period":5}"# },
                });
                if let Some(bar) = provisional {
                    body["provisional"] = bar;
                }
                let response = post_json_auth(
                    format!("http://{addr}/api/indicators/compute"),
                    &token,
                    body,
                )
                .await;
                assert_eq!(response.status(), reqwest::StatusCode::OK);
                body_json(response).await
            }
        };

        let without = compute(None).await;
        let forming_open = range_to; // the bar that opens where the stored ones end
        let with = compute(Some(serde_json::json!({
            "ts_open": forming_open,
            "open": 100,
            "high": 140,
            "low": 100,
            "close": 140,
            "volume": { "kind": "real", "value": 7 },
        })))
        .await;

        let plain = without["display"][0]["points"].as_array().unwrap();
        let extended = with["display"][0]["points"].as_array().unwrap();
        assert_eq!(
            extended.len(),
            plain.len() + 1,
            "the forming bar must contribute exactly one more point"
        );
        assert_eq!(
            extended.last().unwrap()["ts_open"].as_i64().unwrap(),
            forming_open,
            "and that point must sit on the forming bar, not before it"
        );
        // The fake venue's bars all close at 100; a 5-period SMA whose newest
        // input is 140 must have moved off 100, or the provisional bar was
        // accepted and then ignored.
        let last_value = extended.last().unwrap()["value"].as_f64().unwrap();
        assert!(
            (last_value - 100.0).abs() > f64::EPSILON,
            "the provisional close must actually feed the indicator, got {last_value}"
        );

        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn an_unknown_indicator_name_is_a_400_not_a_500() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;
        let instrument = test_instrument();

        let response = post_json_auth(
            format!("http://{addr}/api/indicators/compute"),
            &token,
            serde_json::json!({
                "instrument": instrument,
                "spec": "1m",
                "from": 0,
                "to": 60_000_000_000i64,
                "indicator": { "name": "NotReal", "params": "{}" },
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

        handle.shutdown().await.unwrap();
    }

    /// The built-ins today only ever emit `Series`, so this drives the
    /// conversion directly rather than through `POST
    /// /api/indicators/compute` — proving the response shape holds for a
    /// zone or label indicator before one exists, not after.
    #[test]
    fn every_drawable_kind_survives_the_response_conversion() {
        use senken_indicators::{
            DisplayList, Drawable, Extend, LabelAnchor, Point, PriceCoord, ScaledPrice, SeriesShape,
        };

        let mut display = DisplayList::new(10);
        display.push(Drawable::Series {
            field: "value".to_owned(),
            shape: SeriesShape::Line,
            points: vec![Point {
                time: 1,
                value: 2.0,
            }],
        });
        display.push(Drawable::Segment {
            a: Point {
                time: 1,
                value: 1.0,
            },
            b: Point {
                time: 2,
                value: 2.0,
            },
            extend: Extend::Forward,
        });
        display.push(Drawable::Level {
            price: PriceCoord::Annotation(101.5),
            extend: Extend::Both,
        });
        display.push(Drawable::Level {
            price: PriceCoord::Executable(ScaledPrice {
                value: 100_050,
                scale: 2,
            }),
            extend: Extend::None,
        });
        display.push(Drawable::Box {
            a: Point {
                time: 1,
                value: 1.0,
            },
            b: Point {
                time: 2,
                value: 2.0,
            },
        });
        display.push(Drawable::Label {
            at: Point {
                time: 1,
                value: 1.0,
            },
            text: "pivot".to_owned(),
            anchor: LabelAnchor::Above,
        });

        let dtos: Vec<IndicatorDrawableDto> =
            display.drawables().map(super::drawable_dto).collect();
        assert_eq!(
            dtos.len(),
            6,
            "every kind pushed must reach the response — a wildcard match \
             arm silently drops whichever kinds it does not name"
        );
        assert!(matches!(dtos[0], IndicatorDrawableDto::Series { .. }));
        assert!(matches!(dtos[1], IndicatorDrawableDto::Segment { .. }));
        assert!(matches!(dtos[2], IndicatorDrawableDto::Level { .. }));
        assert!(matches!(dtos[3], IndicatorDrawableDto::Level { .. }));
        assert!(matches!(dtos[4], IndicatorDrawableDto::Box { .. }));
        assert!(matches!(dtos[5], IndicatorDrawableDto::Label { .. }));
    }

    /// Every built-in descriptor must declare a positive per-item display
    /// cap. `DisplayList` exempts `Series` from its cap, which is exactly
    /// why a cap of zero went unnoticed: nothing built in today emits a
    /// non-series object, so nothing today would have shown the bug.
    #[test]
    fn every_descriptor_declares_a_positive_display_cap() {
        for descriptor in senken_indicators::DESCRIPTORS {
            assert!(
                descriptor.max_display_objects > 0,
                "{} has a zero per-item display cap: any non-series object \
                 it ever emits would be discarded before a response leaves \
                 the server",
                descriptor.id
            );
        }
    }

    /// Demonstrates, without relying on any built-in emitting non-series
    /// geometry yet, exactly what a per-item cap of zero does versus a real
    /// one — the failure mode `compute_indicator` used to have silently.
    #[test]
    fn a_zero_display_cap_discards_every_non_series_object_a_real_cap_would_keep() {
        use senken_indicators::{DisplayList, Drawable, Point};

        let box_drawable = |i: i64| Drawable::Box {
            a: Point {
                time: i,
                value: 1.0,
            },
            b: Point {
                time: i + 1,
                value: 2.0,
            },
        };

        let mut real_cap = DisplayList::new(3);
        let mut zero_cap = DisplayList::new(0);
        for i in 0..3 {
            real_cap.push(box_drawable(i));
            zero_cap.push(box_drawable(i));
        }

        assert_eq!(
            real_cap.drawables().count(),
            3,
            "a real cap retains what fits under it"
        );
        assert_eq!(
            zero_cap.drawables().count(),
            0,
            "a cap of zero — DisplayList::new(0) — discards every one"
        );
        assert_eq!(zero_cap.discarded_objects(), 3);
    }

    /// Ensures `range` is resolvable and returns `(instrument, from, to)` —
    /// the same `ensure` -> poll shape every other test in this module
    /// duplicates inline; factored out once here since the dynamic-plugin
    /// tests below need it three times.
    async fn ensure_range(addr: std::net::SocketAddr, token: &str) -> (String, i64, i64) {
        let instrument = test_instrument();
        let range_to: i64 = 20 * 60 * 1_000_000_000; // 20 one-minute bars
        let ensure = post_json_auth(
            format!("http://{addr}/api/bars/ensure"),
            token,
            serde_json::json!({ "instrument": instrument, "spec": "1m", "from": 0, "to": range_to }),
        )
        .await;
        let job_id = body_json(ensure).await["job_id"]
            .as_str()
            .unwrap()
            .to_owned();
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let response =
                    get_auth(format!("http://{addr}/api/bars/jobs/{job_id}"), token).await;
                if body_json(response).await["phase"] == "done" {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        (instrument, 0, range_to)
    }

    /// The full path this slice exists to prove, over real HTTP: an
    /// uploaded `.wasm` indicator appears in the catalogue, and its
    /// computed value matches the equivalent native built-in bar for bar —
    /// both call the exact same compiled `Ema`, one directly and one
    /// through `wit/senken.wit`'s `builtins` import, so a mismatch here
    /// would mean the bridge (`senken_runtime::plugin_host`), not the
    /// maths, is wrong.
    #[tokio::test]
    async fn an_uploaded_wasm_indicator_appears_in_the_catalogue_and_matches_the_native_equivalent()
    {
        let wasm = build_dynamic_indicator_fixture("dyn-ema");
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let upload = post_bytes_auth(
            format!("http://{addr}/api/indicators/plugins"),
            &token,
            "application/wasm",
            wasm,
        )
        .await;
        assert_eq!(upload.status(), reqwest::StatusCode::OK);
        let uploaded = body_json(upload).await;
        assert_eq!(uploaded["name"], "DynEma");

        let catalogue =
            body_json(get_auth(format!("http://{addr}/api/indicators"), &token).await).await;
        let names: Vec<&str> = catalogue
            .as_array()
            .unwrap()
            .iter()
            .map(|entry| entry["name"].as_str().unwrap())
            .collect();
        assert!(
            names.contains(&"DynEma"),
            "the uploaded plugin must join the built-ins in the catalogue: {names:?}"
        );

        let (instrument, from, to) = ensure_range(addr, &token).await;
        let compute = |name: &'static str| {
            let token = token.clone();
            let instrument = instrument.clone();
            async move {
                let response = post_json_auth(
                    format!("http://{addr}/api/indicators/compute"),
                    &token,
                    serde_json::json!({
                        "instrument": instrument,
                        "spec": "1m",
                        "from": from,
                        "to": to,
                        "indicator": { "name": name, "params": r#"{"period":5}"# },
                    }),
                )
                .await;
                assert_eq!(
                    response.status(),
                    reqwest::StatusCode::OK,
                    "computing {name}"
                );
                body_json(response).await
            }
        };

        let native = compute("Ema").await;
        let dynamic = compute("DynEma").await;
        let native_points = native["display"][0]["points"].as_array().unwrap();
        let dynamic_points = dynamic["display"][0]["points"].as_array().unwrap();
        assert_eq!(
            native_points.len(),
            dynamic_points.len(),
            "both computed over the exact same resolved bar range"
        );
        assert!(!native_points.is_empty());
        for (native_point, dynamic_point) in native_points.iter().zip(dynamic_points) {
            assert_eq!(native_point["ts_open"], dynamic_point["ts_open"]);
            let native_value = native_point["value"].as_f64().unwrap();
            let dynamic_value = dynamic_point["value"].as_f64().unwrap();
            assert!(
                (native_value - dynamic_value).abs() < 1e-9,
                "native Ema {native_value} and DynEma {dynamic_value} must match bar-for-bar"
            );
        }

        handle.shutdown().await.unwrap();
    }

    /// Disabling an uploaded plugin removes it from the catalogue and
    /// refuses a compute call naming it, without discarding the
    /// registration; re-enabling restores both — the whole point of a
    /// placeholder rather than a deletion.
    #[tokio::test]
    async fn disabling_an_uploaded_plugin_removes_it_and_enabling_restores_it() {
        let wasm = build_dynamic_indicator_fixture("dyn-ema");
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        post_bytes_auth(
            format!("http://{addr}/api/indicators/plugins"),
            &token,
            "application/wasm",
            wasm,
        )
        .await;
        let (instrument, from, to) = ensure_range(addr, &token).await;
        let compute_body = serde_json::json!({
            "instrument": instrument,
            "spec": "1m",
            "from": from,
            "to": to,
            "indicator": { "name": "DynEma", "params": r#"{"period":5}"# },
        });

        let disable = post_json_auth(
            format!("http://{addr}/api/indicators/plugins/DynEma/enabled"),
            &token,
            serde_json::json!({ "enabled": false }),
        )
        .await;
        assert_eq!(disable.status(), reqwest::StatusCode::OK);

        let catalogue =
            body_json(get_auth(format!("http://{addr}/api/indicators"), &token).await).await;
        assert!(
            catalogue
                .as_array()
                .unwrap()
                .iter()
                .all(|entry| entry["name"] != "DynEma"),
            "a disabled plugin must not appear in the catalogue"
        );
        let refused = post_json_auth(
            format!("http://{addr}/api/indicators/compute"),
            &token,
            compute_body.clone(),
        )
        .await;
        assert_eq!(
            refused.status(),
            reqwest::StatusCode::BAD_REQUEST,
            "a disabled indicator must not compute"
        );

        // The registration itself survives — `GET /api/indicators/plugins`
        // (unlike `GET /api/indicators`) reports every registered plugin
        // regardless of enabled state.
        let plugins =
            body_json(get_auth(format!("http://{addr}/api/indicators/plugins"), &token).await)
                .await;
        let entry = plugins
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["name"] == "DynEma")
            .expect("a disabled plugin is still listed here, with state \"disabled\"");
        assert_eq!(entry["state"], "disabled");

        let enable = post_json_auth(
            format!("http://{addr}/api/indicators/plugins/DynEma/enabled"),
            &token,
            serde_json::json!({ "enabled": true }),
        )
        .await;
        assert_eq!(enable.status(), reqwest::StatusCode::OK);

        let restored_catalogue =
            body_json(get_auth(format!("http://{addr}/api/indicators"), &token).await).await;
        assert!(
            restored_catalogue
                .as_array()
                .unwrap()
                .iter()
                .any(|entry| entry["name"] == "DynEma"),
            "enabling again must restore the catalogue entry"
        );
        let restored_compute = post_json_auth(
            format!("http://{addr}/api/indicators/compute"),
            &token,
            compute_body,
        )
        .await;
        assert_eq!(
            restored_compute.status(),
            reqwest::StatusCode::OK,
            "and computing it again must work, with the same params as before"
        );

        handle.shutdown().await.unwrap();
    }

    /// A plugin that produces more display objects than the host allows is
    /// rejected outright, with a message naming it — never a chart that
    /// quietly stops drawing some of its objects.
    #[tokio::test]
    async fn an_overloaded_plugin_is_rejected_with_a_message_not_silently_truncated() {
        let wasm = build_dynamic_indicator_fixture("dyn-overload");
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        post_bytes_auth(
            format!("http://{addr}/api/indicators/plugins"),
            &token,
            "application/wasm",
            wasm,
        )
        .await;
        // 50 `Level`s per bar (the fixture) over 20 bars is 1000 — well
        // past the 500-object cap.
        let (instrument, from, to) = ensure_range(addr, &token).await;
        let response = post_json_auth(
            format!("http://{addr}/api/indicators/compute"),
            &token,
            serde_json::json!({
                "instrument": instrument,
                "spec": "1m",
                "from": from,
                "to": to,
                "indicator": { "name": "DynOverload", "params": "{}" },
            }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        let message = body["error"].as_str().unwrap_or_default();
        assert!(
            message.contains("DynOverload") && message.contains("display objects"),
            "the rejection must name the offending indicator and explain why: {message:?}"
        );

        handle.shutdown().await.unwrap();
    }

    /// Hiding the upload/list/enable controls in the Plugins page UI is not
    /// access control — every one of the three `/indicators/plugins*`
    /// routes must refuse a request that carries no session at all with a
    /// `401`, never a `200` reached by skipping straight past the browser.
    #[tokio::test]
    async fn requests_with_no_credentials_get_401_on_every_plugin_route() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();

        let upload = reqwest::Client::new()
            .post(format!("http://{addr}/api/indicators/plugins"))
            .header("content-type", "application/wasm")
            .body(vec![0u8; 4])
            .send()
            .await
            .unwrap();
        assert_eq!(upload.status(), reqwest::StatusCode::UNAUTHORIZED);

        let list = reqwest::get(format!("http://{addr}/api/indicators/plugins"))
            .await
            .unwrap();
        assert_eq!(list.status(), reqwest::StatusCode::UNAUTHORIZED);

        let enable = reqwest::Client::new()
            .post(format!(
                "http://{addr}/api/indicators/plugins/DynEma/enabled"
            ))
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&serde_json::json!({ "enabled": true })).unwrap())
            .send()
            .await
            .unwrap();
        assert_eq!(enable.status(), reqwest::StatusCode::UNAUTHORIZED);

        handle.shutdown().await.unwrap();
    }

    /// A signed-in user with no grant on `Resource::Indicator` must be
    /// refused the same way on all three routes — `403`, not `401`, and the
    /// refusal must not be a disguised logout: the same token still resolves
    /// a plain authenticated request afterward.
    #[tokio::test]
    async fn a_signed_in_user_without_the_indicator_grant_gets_403_and_keeps_their_session() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        identity
            .create_user(
                &admin,
                "noplugins@example.com",
                "No Plugins",
                Some("a very long password"),
            )
            .unwrap();
        let token = login_token(addr, "noplugins@example.com", "a very long password").await;

        let upload = post_bytes_auth(
            format!("http://{addr}/api/indicators/plugins"),
            &token,
            "application/wasm",
            vec![0u8; 4],
        )
        .await;
        assert_eq!(
            upload.status(),
            reqwest::StatusCode::FORBIDDEN,
            "a valid session with no Indicator grant must be 403, never a logout-triggering 401"
        );

        let list = get_auth(format!("http://{addr}/api/indicators/plugins"), &token).await;
        assert_eq!(list.status(), reqwest::StatusCode::FORBIDDEN);

        let enable = post_json_auth(
            format!("http://{addr}/api/indicators/plugins/DynEma/enabled"),
            &token,
            serde_json::json!({ "enabled": true }),
        )
        .await;
        assert_eq!(enable.status(), reqwest::StatusCode::FORBIDDEN);

        // A `403` must never clear the credential: the very same token
        // still resolves an ordinary authenticated request right after.
        let me = get_auth(format!("http://{addr}/api/me"), &token).await;
        assert_eq!(
            me.status(),
            reqwest::StatusCode::OK,
            "three 403s in a row must not have logged this session out"
        );

        handle.shutdown().await.unwrap();
    }

    /// [`admin_token`]'s counterpart for an arbitrary email/password —
    /// `admin_token` is fixed to the seeded superadmin, and the 403 test
    /// above needs a second, unprivileged account.
    async fn login_token(addr: std::net::SocketAddr, email: &str, password: &str) -> String {
        let response = post_json(
            format!("http://{addr}/api/login"),
            serde_json::json!({ "email": email, "password": password }),
        )
        .await;
        body_json(response).await["token"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// The authoring panel needs a mistake's exact line and column to place
    /// its own error marker — a flattened `{"error": "..."}` string would
    /// force it to re-parse the compiler's own prose. This proves the `400`
    /// body carries them as separate fields, not folded into one message,
    /// and that the message itself is the compiler's own, not a generic
    /// wrapper's.
    #[tokio::test]
    async fn a_syntax_mistake_in_compiled_source_reports_its_own_line_and_column_not_a_generic_message()
     {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        // Two lines so the reported line is proof the compiler's own count
        // is reaching the response, not a hard-coded `1`: the mistake (a
        // dangling operator with nothing after it) sits on line 2.
        let source = "let fast = ema(close, 12)\nplot fast +";
        let response = post_json_auth(
            format!("http://{addr}/api/indicators/compile"),
            &token,
            serde_json::json!({ "source": source }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);
        let body = body_json(response).await;
        assert_eq!(
            body["line"].as_u64(),
            Some(2),
            "the mistake is on the second line: {body:?}"
        );
        assert!(
            body["column"].as_u64().is_some(),
            "a column must be reported: {body:?}"
        );
        assert!(
            body.get("error").is_none(),
            "a compile error is line/column/message, never the crate-wide `error` shape: {body:?}"
        );
        let message = body["message"].as_str().unwrap();
        assert!(!message.is_empty());

        handle.shutdown().await.unwrap();
    }

    /// A program with no mistake in it must get *past* the compiler and
    /// register successfully: `senken_indicator_lang::compile` targets
    /// `wit/senken.wit`'s `compiled-indicator` world (a bare `on-bar`
    /// export, no descriptor, no `indicator` interface — see that crate's
    /// own `README.md`), and `senken_runtime::DynamicIndicators::register`
    /// now bridges that world into a dynamic indicator the same way it
    /// already does for a Rust-authored `indicator-plugin` component,
    /// synthesising the catalogue entry's id/title/plot from the compiled
    /// bytes since the language itself has no syntax for any of them. This
    /// used to fail here — `register` had no path for a `compiled-indicator`
    /// artifact at all and always answered `PluginHostError::Load`,
    /// regardless of how clean the source was — so this asserts the `200`
    /// that failure mode's own pinned comment said this test would become.
    #[tokio::test]
    async fn a_valid_program_gets_past_the_compiler_and_fails_only_at_registration() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, _store, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let token = admin_token(addr).await;

        let response = post_json_auth(
            format!("http://{addr}/api/indicators/compile"),
            &token,
            serde_json::json!({ "source": "plot close" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::OK);
        let body = body_json(response).await;
        assert!(
            body.get("line").is_none() && body.get("column").is_none(),
            "a clean program must never be reported as a source mistake: {body:?}"
        );
        let name = body["name"]
            .as_str()
            .expect("a successful compile registers under the `IndicatorCatalogEntry` shape");
        assert!(
            !name.is_empty(),
            "the synthesised catalogue entry must carry a real id: {body:?}"
        );
        assert_eq!(
            body["params"].as_array().map(Vec::len),
            Some(0),
            "a compiled program has no runtime-configurable parameters: {body:?}"
        );

        handle.shutdown().await.unwrap();
    }

    /// `POST /api/indicators/compile` mutates the same shared catalogue an
    /// upload does, so it is guarded the same way — a session with no
    /// grant on `Resource::Indicator` gets `403`, never a `500` from
    /// reaching the compiler at all.
    #[tokio::test]
    async fn compiling_without_the_indicator_grant_is_403() {
        let runtime_dir = tempfile::TempDir::new().unwrap();
        let (runtime, _bar_source) = runtime_with_fake_venue(runtime_dir.path());
        let (handle, identity, _dir) = serve_unfenced_test_server_with(runtime).await;
        let addr = handle.local_addr();
        let (_uid, admin_session) = identity
            .login(DEFAULT_ADMIN_EMAIL, ADMIN_TEST_PASSWORD)
            .unwrap();
        let admin = identity
            .resolve_session(admin_session.reveal())
            .unwrap()
            .unwrap();
        identity
            .create_user(
                &admin,
                "nocompile@example.com",
                "No Compile",
                Some("a very long password"),
            )
            .unwrap();
        let token = login_token(addr, "nocompile@example.com", "a very long password").await;

        let response = post_json_auth(
            format!("http://{addr}/api/indicators/compile"),
            &token,
            serde_json::json!({ "source": "plot close" }),
        )
        .await;
        assert_eq!(response.status(), reqwest::StatusCode::FORBIDDEN);

        handle.shutdown().await.unwrap();
    }
}
