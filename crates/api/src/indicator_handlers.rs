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
use senken_series::Origin;

use crate::AppState;
use crate::HandlerError;
use crate::auth::Authed;
use crate::dto::{
    BarRangeQuery, ComputeIndicatorRequest, ComputeIndicatorResponse, IndicatorCatalogEntry,
    IndicatorFieldValue, IndicatorPointDto,
};

/// One catalogue entry. `name` matches exactly what
/// `senken_alerts::IndicatorSpec::build`/`ConcreteIndicator::build` accepts
/// (case-insensitively) — see [`the_catalogues_names_are_exactly_what_the_indicator_factory_accepts`]
/// for the test that keeps the two from drifting apart.
fn entry(name: &str, params: &[&str], fields: &[&str]) -> IndicatorCatalogEntry {
    IndicatorCatalogEntry {
        name: name.to_owned(),
        params: params.iter().map(|s| (*s).to_owned()).collect(),
        fields: fields.iter().map(|s| (*s).to_owned()).collect(),
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
    Json(vec![
        entry("Sma", &["period"], &["value"]),
        entry("Ema", &["period"], &["value"]),
        entry("Wma", &["period"], &["value"]),
        entry("Rsi", &["period"], &["value"]),
        entry("Atr", &["period"], &["value"]),
        entry("Vwap", &[], &["value"]),
        entry("Volume", &[], &["value"]),
        entry(
            "Macd",
            &["fast_period", "slow_period", "signal_period"],
            &["macd_line", "macd_signal", "macd_histogram"],
        ),
        entry(
            "Stochastic",
            &["k_period", "d_period"],
            &["stochastic_k", "stochastic_d"],
        ),
        entry(
            "BollingerBands",
            &["period", "k"],
            &["bollinger_upper", "bollinger_middle", "bollinger_lower"],
        ),
    ])
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
    let key = senken_series::SeriesKey::new(id.source(), id.symbol(), Origin::Derived, spec);
    let resolved = loader
        .resolve(&key, range, senken_series::Anchor::UTC)
        .await
        .map_err(|source| {
            tracing::error!(%source, "bars resolve failed while computing an indicator");
            HandlerError::Internal
        })?;

    let indicator_spec = IndicatorSpec::from(body.indicator);
    let mut indicator = indicator_spec
        .build()
        .map_err(|source| HandlerError::BadRequest(source.to_string()))?;
    let fields = reported_fields(&indicator);

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
        volume: dto.volume,
        quote_volume: None,
        trade_count: None,
        taker_buy_volume: None,
    });

    let mut points = Vec::new();
    for bar in resolved.bars.iter().chain(provisional.iter()) {
        indicator.handle_bar(bar);
        if !indicator.initialized() {
            continue;
        }
        let values = fields
            .iter()
            .filter_map(|field| {
                indicator
                    .read(*field)
                    .ok()
                    .map(|value| IndicatorFieldValue {
                        field: field_key(*field).to_owned(),
                        value,
                    })
            })
            .collect();
        points.push(IndicatorPointDto {
            ts_open: bar.ts_open.as_nanos(),
            values,
        });
    }

    Ok(Json(ComputeIndicatorResponse {
        points,
        missing: resolved.missing.into_iter().map(Into::into).collect(),
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
                    .map(|p| (p.as_str().unwrap().to_owned(), serde_json::json!(5)))
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
        let points = body["points"].as_array().unwrap();
        // The fake venue's bars all close at 100 (see
        // `bars_handlers::test_support::m1_bars_for`), so a fully warmed-up
        // SMA reports exactly 100 — and only from the 5th bar onward.
        assert_eq!(
            points.len(),
            20 - 4,
            "the first period-1 bars report no value yet"
        );
        for point in points {
            let values = point["values"].as_array().unwrap();
            assert_eq!(values.len(), 1);
            assert_eq!(values[0]["field"], "value");
            assert!((values[0]["value"].as_f64().unwrap() - 100.0).abs() < f64::EPSILON);
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
            "volume": 7,
        })))
        .await;

        let plain = without["points"].as_array().unwrap();
        let extended = with["points"].as_array().unwrap();
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
        let last_value = extended.last().unwrap()["values"][0]["value"]
            .as_f64()
            .unwrap();
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
