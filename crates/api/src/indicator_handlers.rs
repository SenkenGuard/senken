//! Indicators over HTTP.
//!
//! `GET /api/indicators` lists the ten built-ins `senken-indicators`
//! implements; `POST /api/indicators/compute` evaluates one of them over a
//! bar range already resolvable through `SeriesLoader::resolve` (the same
//! read path `bars_handlers::range_bars` uses — this endpoint fetches
//! nothing itself, so a caller must have already `POST
//! /api/bars/ensure`d the range it asks about). Building and reading the
//! concrete indicator reuses `senken_alerts::{IndicatorSpec,
//! ConcreteIndicator}` rather than a second factory — an alert and a chart
//! layer are just two consumers of the same ten built-ins (the //! whole point: one source of truth for the maths, not the browser's own
//! copy).

use axum::extract::State;
use axum::{Extension, Json};

use senken_alerts::{ConcreteIndicator, IndicatorField, IndicatorSpec};
use senken_indicators::{
    DESCRIPTORS, DisplayList, Drawable, IndicatorDescriptor, ParamDefault, ParamKind, Placement,
    PlotShape, Point, ScaleHint, SeriesShape, VolumeRequirement, descriptor,
};
use senken_series::Origin;

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    BarRangeQuery, ComputeIndicatorRequest, ComputeIndicatorResponse, IndicatorCatalogEntry,
    IndicatorDrawableDto, IndicatorDrawablePointDto, IndicatorParamDefaultDto, IndicatorParamDto,
    IndicatorPlacementDto, IndicatorPlotDto, IndicatorScaleDto,
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

/// The wire name for one [`IndicatorField`] — matches
/// `senken_alerts`' own private `encode_condition` string table (an alert's
/// `condition_field` column), so a client sees one vocabulary for "which
/// number does this indicator report" everywhere it appears.
fn field_key(field: IndicatorField) -> &'static str {
    match field {
        IndicatorField::Value => "value",
        IndicatorField::MacdLine => "macd_line",
        IndicatorField::MacdSignal => "macd_signal",
        IndicatorField::MacdHistogram => "macd_histogram",
        IndicatorField::StochasticK => "stochastic_k",
        IndicatorField::StochasticD => "stochastic_d",
        IndicatorField::BollingerUpper => "bollinger_upper",
        IndicatorField::BollingerMiddle => "bollinger_middle",
        IndicatorField::BollingerLower => "bollinger_lower",
    }
}

/// Which [`IndicatorField`]s a built [`ConcreteIndicator`] actually reports
///   — an exhaustive match (not a name lookup) so a new variant added to
/// either enum is a compile error here, not a silently wrong catalogue.
fn reported_fields(indicator: &ConcreteIndicator) -> &'static [IndicatorField] {
    use IndicatorField as F;
    match indicator {
        ConcreteIndicator::Sma(_)
        | ConcreteIndicator::Ema(_)
        | ConcreteIndicator::Wma(_)
        | ConcreteIndicator::Rsi(_)
        | ConcreteIndicator::Atr(_)
        | ConcreteIndicator::Vwap(_)
        | ConcreteIndicator::Volume(_) => &[F::Value],
        ConcreteIndicator::Macd(_) => &[F::MacdLine, F::MacdSignal, F::MacdHistogram],
        ConcreteIndicator::Stochastic(_) => &[F::StochasticK, F::StochasticD],
        ConcreteIndicator::BollingerBands(_) => {
            &[F::BollingerUpper, F::BollingerMiddle, F::BollingerLower]
        }
    }
}

fn display_for_indicator(
    indicator: &mut ConcreteIndicator,
    descriptor: &IndicatorDescriptor,
    bars: &[senken_series::Bar],
    provisional: Option<senken_series::Bar>,
    start: senken_core::UnixNanos,
) -> (Vec<IndicatorDrawableDto>, usize) {
    let mut fields: Vec<(IndicatorField, Vec<IndicatorDrawablePointDto>)> =
        reported_fields(indicator)
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

    let mut display = DisplayList::new(0);
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
    let display = display
        .drawables()
        .filter_map(|drawable| match drawable {
            Drawable::Series {
                field,
                shape,
                points,
            } => Some(IndicatorDrawableDto::Series {
                field: field.clone(),
                shape: match shape {
                    SeriesShape::Line => "line",
                    SeriesShape::Histogram => "histogram",
                    SeriesShape::Area => "area",
                }
                .to_owned(),
                points: points
                    .iter()
                    .map(|point| IndicatorDrawablePointDto {
                        ts_open: point.time,
                        value: point.value,
                    })
                    .collect(),
            }),
            _ => None,
        })
        .collect();
    (display, discarded_objects)
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
    let parameter_values: serde_json::Value = serde_json::from_str(&body.indicator.params)
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
    let resolve_range = senken_core::TimeRange::new(
        senken_core::UnixNanos::from_nanos(prefix_start),
        range.end(),
    )
    .ok_or(HandlerError::BadRequest(
        "invalid indicator range".to_owned(),
    ))?;
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

    let warmup_truncated = prefix_start != range.start().as_nanos()
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
            // catalogue and `senken_alerts::ConcreteIndicator::build`
            // cannot silently drift apart.
            let json = serde_json::Value::Object(
                params
                    .iter()
                    .map(|p| (p["name"].as_str().unwrap().to_owned(), serde_json::json!(5)))
                    .collect(),
            );
            senken_alerts::ConcreteIndicator::build(name, &json.to_string())
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
}
