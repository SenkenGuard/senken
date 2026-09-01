//! Indicators over HTTP.
//!
//! `GET /api/indicators` lists the ten built-ins `senken-indicators`
//! implements; `POST /api/indicators/compute` evaluates one of them over a
//! bar range already resolvable through `SeriesLoader::resolve` (the same
//! read path `bars_handlers::range_bars` uses — this endpoint fetches
//! nothing itself, so a caller must have already `POST
//! /api/bars/ensure`d the range it asks about). Building and reading the
//! concrete indicator reuses `senken_indicators::ConcreteIndicator` — the
//! dynamic build-and-read contract every consumer of the ten built-ins
//! shares (an alert row, a workspace layer, the live indicator sessions
//! `ws` drives) — rather than a second factory: one source of truth for the
//! maths, not the browser's own copy.

use axum::extract::State;
use axum::{Extension, Json};

use senken_alerts::IndicatorSpec;
use senken_core::TimeRange;
use senken_indicators::{
    ConcreteIndicator, DESCRIPTORS, DisplayList, Drawable, Extend, IndicatorDescriptor,
    IndicatorField, LabelAnchor, ParamDefault, ParamKind, Placement, PlotShape, Point, PriceCoord,
    ScaleHint, ScaledPrice, SeriesShape, VolumeRequirement, descriptor,
};
use senken_series::{BarSpec, Origin};

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    BarRangeQuery, ComputeIndicatorRequest, ComputeIndicatorResponse, IndicatorCatalogEntry,
    IndicatorDrawableDto, IndicatorDrawablePointDto, IndicatorExtendDto, IndicatorLabelAnchorDto,
    IndicatorParamDefaultDto, IndicatorParamDto, IndicatorPlacementDto, IndicatorPlotDto,
    IndicatorPointDto, IndicatorPriceCoordDto, IndicatorScaleDto, IndicatorScaledPriceDto,
};

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

/// `GET /api/indicators`: the catalogue of
/// `senken-indicators`' ten built-ins.
#[utoipa::path(
    get,
    path = "/api/indicators",
    responses(
        (status = 200, body = Vec<IndicatorCatalogEntry>),
        (status = 401, body = crate::dto::ErrorBody),
    )
)]
pub(crate) async fn list_indicators(Extension(_ctx): Authed) -> Json<Vec<IndicatorCatalogEntry>> {
    Json(DESCRIPTORS.iter().map(entry).collect())
}

/// `POST /api/indicators/compute`: replays
/// whatever bars are already resolvable for `instrument`/`spec`/`from`/`to`
/// through the named indicator, one bar at a time — the same incremental
/// discipline `senken-indicators` itself is built on ("one code path, live or backfilled") — and reports one point per bar once the
/// indicator is `initialized()` (never a warm-up value).
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
    let descriptor = descriptor(&body.indicator.name)
        .ok_or_else(|| HandlerError::BadRequest("unknown indicator".to_owned()))?;
    let resolve_range = warmup_extended_range(descriptor, spec, &body.indicator.params, range)?;
    let key = senken_series::SeriesKey::new(id.source(), id.symbol(), Origin::Derived, spec);
    let resolved = loader
        .resolve(&key, resolve_range, senken_series::Anchor::UTC)
        .await
        .map_err(|source| {
            tracing::error!(%source, "bars resolve failed while computing an indicator");
            HandlerError::Internal
        })?;

    let indicator_spec = IndicatorSpec::from(body.indicator);
    let mut indicator = indicator_spec
        .build()
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;

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

#[cfg(test)]
mod tests {
    use senken_identity::DEFAULT_ADMIN_EMAIL;
    use senken_indicators::ConcreteIndicator;

    use crate::dto::IndicatorDrawableDto;

    use crate::bars_handlers::test_support::{runtime_with_fake_venue, test_instrument};
    use crate::test_support::{
        ADMIN_TEST_PASSWORD, body_json, get_auth, post_json, post_json_auth,
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
}
